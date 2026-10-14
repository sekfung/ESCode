/**
 * CCID（USB 智能卡读卡器，接口类 0x0B）解码器，见 docs/specs/usb-agent-tools.md §7。
 * 报文 = 10 字节通用头（bMessageType/dwLength/bSlot/bSeq + 3 字节类型相关字段）+ 载荷：
 * XfrBlock 载荷是 ISO 7816-4 短 APDU，RDR_to_PC_DataBlock 载荷是响应数据 + SW1SW2，IccPowerOn
 * 的响应载荷是 ATR；链式块合并为一次逻辑交换，块级原始字段写进 detail。
 */
import { byteAt, hex, readUint16BE, readUint16LE, readUint32LE, toHex } from "../format.js";
import { USB_CLASS, type DecodeContext, type DecodedMessage, type UsbExchange } from "../types.js";

const HEADER_SIZE = 10;
const MAX_BLOCK_DETAILS = 8;
const TYPE_SECURE = 0x69;
const TYPE_XFR_BLOCK = 0x6f;
const TYPE_DATA_BLOCK = 0x80;
/** ATR 接口字节的存在位（T0/TDi 高半字节）与名字。 */
const ATR_MARKS = [
  [0x80, "TA"],
  [0x40, "TB"],
  [0x20, "TC"],
] as const;

/** 紧凑表：`十六进制码 名称` 以 `|` 分隔。只收录有把握的条目，表外一律 unknown(0xNN)。 */
function parseSpec(spec: string): Record<number, string> {
  const table: Record<number, string> = {};
  for (const item of spec.split("|")) {
    const [code, value] = item.split(" ");
    if (code !== undefined && value !== undefined) table[Number.parseInt(code, 16)] = value;
  }
  return table;
}

/** CCID Rev 1.1 §6.1/§6.2 消息类型。 */
const MESSAGE_NAMES = parseSpec(
  "50 RDR_to_PC_NotifySlotChange|51 RDR_to_PC_HardwareError|61 PC_to_RDR_SetParameters|" +
    "62 PC_to_RDR_IccPowerOn|63 PC_to_RDR_IccPowerOff|65 PC_to_RDR_GetSlotStatus|" +
    "69 PC_to_RDR_Secure|6a PC_to_RDR_T0APDU|6b PC_to_RDR_Escape|6c PC_to_RDR_GetParameters|" +
    "6d PC_to_RDR_ResetParameters|6e PC_to_RDR_IccClock|6f PC_to_RDR_XfrBlock|" +
    "71 PC_to_RDR_Mechanical|72 PC_to_RDR_Abort|73 PC_to_RDR_SetDataRateAndClockFrequency|" +
    "80 RDR_to_PC_DataBlock|81 RDR_to_PC_SlotStatus|82 RDR_to_PC_Parameters|83 RDR_to_PC_Escape|" +
    "84 RDR_to_PC_DataRateAndClockFrequency",
);
/** ISO 7816-4 短 APDU 的 INS。 */
const INS_NAMES = parseSpec(
  "20 VERIFY|70 MANAGE CHANNEL|82 EXTERNAL AUTHENTICATE|84 GET CHALLENGE|88 INTERNAL AUTHENTICATE|" +
    "a4 SELECT|b0 READ BINARY|b2 READ RECORD|c0 GET RESPONSE|ca GET DATA|d0 WRITE BINARY|" +
    "d2 WRITE RECORD|da PUT DATA",
);
/** SW1SW2 逐条收录的含义；0x61xx / 0x6Cxx / 0x63Cx 另有规则。 */
const SW_NAMES = parseSpec(
  "9000 成功|6300 校验失败|6700 长度不对|6982 安全状态不满足|6a82 文件未找到|6a83 记录未找到|" +
    "6a86 P1P2 不对|6d00 指令不支持|6e00 CLA 不支持|6f00 未知错误",
);
const ERROR_NAMES = parseSpec("00 无错误|e0 ICC 电压错误");
const COMMAND_STATUS = parseSpec("0 命令处理成功|1 命令失败");
const ICC_STATUS = parseSpec("0 ICC 在位且激活|1 ICC 在位未激活|2 无 ICC");

/** 逻辑报文块；形状由 parseBlock 的返回值推定，避免两处重复声明同一份字段。 */
type CcidBlock = NonNullable<ReturnType<typeof parseBlock>>;

/** 解 CCID 读卡器：入参是该设备（同一 busDev）的全部逻辑报文，按时间升序。 */
export function decodeCcidMessages(context: DecodeContext): DecodedMessage[] {
  const interfaces = context.device.interfaces;
  const isCcid = interfaces.some((item) => item.classCode === USB_CLASS.smartCard);
  // 描述符已确认不是 CCID 接口的设备整体跳过；只有地址没有描述符时只认已收录的消息类型。
  if (interfaces.length > 0 && !isCcid) return [];
  const blocks = context.exchanges
    // CCID 消息走批量与中断端点；控制传输是描述符与类请求，不属于消息流。
    .filter((exchange) => exchange.type === "bulk" || exchange.type === "interrupt")
    .map((exchange) => parseBlock(exchange, isCcid))
    .filter((block): block is CcidBlock => block !== null);
  return buildMessages(blocks, context.device.busDev);
}

/** 解单个逻辑报文；null 表示它不是可确认的 CCID 消息，调用方原样跳过。 */
function parseBlock(exchange: UsbExchange, deviceIsCcid: boolean) {
  const data = exchange.data;
  const type = byteAt(data, 0);
  if (type === undefined) return null;
  const known = MESSAGE_NAMES[type];
  if (known === undefined && !deviceIsCcid) return null;
  const name = known ?? `unknown(${hex(type)})`;
  const head = `#${exchange.index} ${exchange.direction.toUpperCase()}`;
  const anomalies: string[] = [];
  let detail = `${head}, bMessageType=${hex(type)}（${name}）`;
  // 0x60-0x7F 是 PC_to_RDR_*，0x80-0x8F 是 RDR_to_PC_*。
  const role = known === undefined ? "unknown" : type <= 0x7f ? "request" : "response";
  let payload = data;
  let chain = false;

  if (known === undefined || data.length < HEADER_SIZE) {
    // NotifySlotChange / HardwareError 本来就不是 10 字节通用头，不按截断报。
    if (known === undefined) anomalies.push(`未收录的消息类型 bMessageType=${hex(type)}`);
    else if (type !== 0x50 && type !== 0x51) {
      anomalies.push(`消息头不足 ${HEADER_SIZE} 字节（实际 ${data.length} 字节）`);
    }
    detail += `, 原始字节=${toHex(data, 16)}`;
  } else {
    const expected = role === "request" ? "out" : "in";
    if (exchange.direction !== expected) anomalies.push(`${name} 与 ${expected} 方向不符`);
    const declared = readUint32LE(data, 1) ?? 0;
    const available = data.length - HEADER_SIZE;
    if (declared !== available) anomalies.push(`dwLength=${declared} 与载荷 ${available} 字节不符`);
    payload = data.subarray(HEADER_SIZE, HEADER_SIZE + Math.min(declared, available));
    detail += `, dwLength=${declared}, bSlot=${hex(byteAt(data, 5) ?? 0)}, bSeq=${hex(byteAt(data, 6) ?? 0)}`;

    if (type === TYPE_XFR_BLOCK || type === TYPE_SECURE) {
      const level = readUint16LE(data, 8) ?? 0;
      // 只有 XfrBlock 的 wLevelParameter bit4 是命令链标记；Secure 的同名字段语义不同。
      if (type === TYPE_XFR_BLOCK) chain = (level & 0x0010) !== 0;
      detail += `, bBWI=${hex(byteAt(data, 7) ?? 0)}, wLevelParameter=${hex(level, 4)}`;
    } else if (type >= 0x80 && type <= 0x84) {
      const status = byteAt(data, 7) ?? 0;
      const error = byteAt(data, 8) ?? 0;
      const last = byteAt(data, 9) ?? 0;
      const errorText = ERROR_NAMES[error] ?? "未收录";
      detail += `, bStatus=${hex(status)}（${statusMeaning(status)}）, bError=${hex(error)}（${errorText}）`;
      if (type === TYPE_DATA_BLOCK) {
        chain = (last & 0x01) !== 0;
        detail += `, bChainParameter=${hex(last)}`;
      } else detail += `, 类型相关字节=${hex(last)}`;
      if (error !== 0x00) anomalies.push(`bError=${hex(error)}（${errorText}）`);
      if ((status & 0x03) === 0x01) anomalies.push(`bStatus=${hex(status)} 命令失败`);
    } else detail += `, 类型相关字节=${toHex(data.subarray(7, 10))}`;
  }
  return { exchange, name, role, payload, detail, anomalies, chain };
}

/** 归并链式块并逐条产出消息：请求与响应合成一次逻辑交换，链式块只影响 detail 行数。 */
function buildMessages(blocks: CcidBlock[], busDev: string): DecodedMessage[] {
  const messages: DecodedMessage[] = [];
  let primary: CcidBlock | null = null;
  let reqs: CcidBlock[] = [];
  let resps: CcidBlock[] = [];
  let unitAnomalies: string[] = [];
  // 链是否还开着由该方向最后一块的链位决定，不需要另存一份容易忘记复位的状态。
  const tailOpen = (side: CcidBlock[]): boolean => side[side.length - 1]?.chain === true;
  const close = (): void => {
    const first = primary;
    if (first === null) return;
    if (tailOpen(reqs) && resps.length === 0) unitAnomalies.push("命令链未在抓包内结束");
    if (tailOpen(resps)) unitAnomalies.push("响应链未在抓包内结束（缺少后续响应块）");
    const merged = [...reqs, ...resps].sort((a, b) => a.exchange.index - b.exchange.index);
    const anomalies = [...new Set([...unitAnomalies, ...merged.flatMap((b) => b.anomalies)])];
    const detail = merged.slice(0, MAX_BLOCK_DETAILS).map((b) => b.detail);
    if (merged.length > MAX_BLOCK_DETAILS) {
      detail.push(`…另有 ${merged.length - MAX_BLOCK_DETAILS} 个块`);
    }
    const reqBytes = concatPayloads(reqs);
    const resBytes = concatPayloads(resps);
    const kind = first.name;
    const short = kind.replace(/^(PC_to_RDR_|RDR_to_PC_)/, "");
    let payload: Uint8Array | undefined = reqBytes.length > 0 ? reqBytes : resBytes;
    let summary: string;
    if (reqs.length > 0 && resps.length === 0) anomalies.push(`未捕获到 ${short} 的响应`);

    if (first.role === "unknown") {
      summary = `${kind} 未收录报文（字段未解）`;
    } else if (short === "IccPowerOn") {
      payload = resBytes.length > 0 ? resBytes : undefined;
      if (payload === undefined) {
        anomalies.push("IccPowerOn 响应没有 ATR（检查 bStatus / bError）");
        summary = `${short}（未捕获到 ATR）`;
      } else summary = `${short} ATR: ${decodeAtr(payload, detail, anomalies)}`;
    } else if (short === "XfrBlock" || short === "Secure") {
      if (reqBytes.length === 0) {
        anomalies.push("XfrBlock 载荷为空，无法解 APDU");
        summary = `${short}（无 APDU 载荷）`;
      } else {
        const command = decodeApdu(reqBytes, detail, anomalies);
        const response = resps.length === 0 ? "未捕获到响应" : swText(resBytes, detail, anomalies);
        summary = `${short} ${command} → ${response}`;
        payload = reqBytes;
      }
    } else {
      const status = resps.map((b) => byteAt(b.exchange.data, 7)).find((v) => v !== undefined);
      const tail =
        reqs.length === 0
          ? `${resBytes.length} 字节（无对应请求）`
          : ` bSeq=${hex(byteAt(first.exchange.data, 6) ?? 0)}`;
      const state =
        status === undefined ? "" : ` → bStatus=${hex(status)}（${statusMeaning(status)}）`;
      summary = `${short}${tail}${state}`;
    }
    if (reqs.length > 1 || resps.length > 1)
      detail.push(`链式合并: 请求 ${reqs.length} 块 + 响应 ${resps.length} 块`);
    messages.push({
      exchanges: merged.map((b) => b.exchange.index),
      ts: first.exchange.ts,
      busDev,
      direction: first.exchange.direction,
      kind,
      summary,
      detail,
      anomalies,
      payload,
    });
    primary = null;
  };

  for (const block of blocks) {
    const joins =
      primary !== null &&
      block.role !== "unknown" &&
      (block.role === "request" ? tailOpen(reqs) : reqs.length > 0 || tailOpen(resps));
    if (primary === null || !joins) {
      // 无法归属的块自成一次交换；响应链还开着时等后续块，链没走完由 close 记异常。
      close();
      primary = block;
      reqs = block.role === "request" ? [block] : [];
      resps = block.role === "response" ? [block] : [];
      unitAnomalies = [];
      if (block.role !== "request" && !block.chain) close();
      continue;
    }
    if (block.role === "request") {
      reqs.push(block);
      continue;
    }
    if (tailOpen(reqs)) unitAnomalies.push(`命令链未闭合就收到 ${block.name}，可能抓包丢块`);
    resps.push(block);
    if (!block.chain) close();
  }
  close();
  return messages;
}

function concatPayloads(blocks: CcidBlock[]): Uint8Array {
  let total = 0;
  for (const block of blocks) total += block.payload.length;
  const joined = new Uint8Array(total);
  let offset = 0;
  for (const block of blocks) {
    joined.set(block.payload, offset);
    offset += block.payload.length;
  }
  return joined;
}

function swMeaning(sw: number): string {
  const known = SW_NAMES[sw];
  if (known !== undefined) return known;
  const sw1 = sw >> 8;
  const sw2 = sw & 0xff;
  if (sw1 === 0x61) return `还有 ${sw2} 字节可用（需 GET RESPONSE）`;
  if (sw1 === 0x6c) return `Le 不对，实际长度 ${sw2}`;
  if (sw1 === 0x63 && (sw2 & 0xf0) === 0xc0) return `校验失败，剩余尝试次数 ${sw2 & 0x0f}`;
  return `未收录(${hex(sw, 4)})`;
}

/** 取响应尾部 SW1SW2：数据块载荷 = 响应数据 + SW1SW2，链式响应只有最后一块带 SW。 */
function swText(resBytes: Uint8Array, detail: string[], anomalies: string[]): string {
  if (resBytes.length < 2) {
    anomalies.push(`响应载荷不足 2 字节（实际 ${resBytes.length} 字节），无法取 SW1SW2`);
    return `响应 ${resBytes.length} 字节（无 SW1SW2）`;
  }
  const sw = readUint16BE(resBytes, resBytes.length - 2) ?? 0;
  const meaning = swMeaning(sw);
  detail.push(`响应: 数据 ${resBytes.length - 2} 字节, SW1SW2=${hex(sw, 4)}（${meaning}）`);
  if (sw !== 0x9000) anomalies.push(`SW1SW2=${hex(sw, 4)}（${meaning}）`);
  return `${hex(sw, 4)}（${meaning}）`;
}

function statusMeaning(status: number): string {
  const command = COMMAND_STATUS[status & 0x03] ?? `命令状态未收录(${hex(status & 0x03)})`;
  const icc = ICC_STATUS[(status >> 6) & 0x03] ?? `ICC 状态未收录(${hex((status >> 6) & 0x03)})`;
  return `${command}, ${icc}`;
}

/** 解 ISO 7816-4 短 APDU；返回命令名，字段行与 anomaly 追加到调用方数组。 */
function decodeApdu(apdu: Uint8Array, detail: string[], anomalies: string[]): string {
  const cla = byteAt(apdu, 0);
  const ins = byteAt(apdu, 1);
  const p1 = byteAt(apdu, 2);
  const p2 = byteAt(apdu, 3);
  if (cla === undefined || ins === undefined || p1 === undefined || p2 === undefined) {
    anomalies.push(`APDU 不足 4 字节（实际 ${apdu.length} 字节），无法解 CLA/INS/P1/P2`);
    detail.push(`APDU 原始字节=${toHex(apdu, 16)}`);
    return "APDU";
  }
  const insText = INS_NAMES[ins] ?? "未收录";
  const name = INS_NAMES[ins] ?? `INS ${hex(ins)}（未收录）`;
  detail.push(`APDU: CLA=${hex(cla)} INS=${hex(ins)} ${insText} P1=${hex(p1)} P2=${hex(p2)}`);
  const body = apdu.subarray(4);
  const lc = byteAt(body, 0);
  const le = (value: number): string => (value === 0 ? "0x00(即 256)" : String(value));
  if (lc === undefined) detail.push("APDU: 无 Lc/Le（case 1）");
  else if (body.length === 1) detail.push(`APDU: Le=${le(lc)}（case 2）`);
  else if (lc === 0) {
    anomalies.push(`APDU 以 0x00 开头且仍有 ${body.length} 字节：疑似扩展长度，未按短 APDU 解码`);
  } else if (body.length === lc + 1) {
    detail.push(`APDU: Lc=${lc}, data=${toHex(body.subarray(1), 16)}（case 3）`);
  } else if (body.length === lc + 2) {
    const data = toHex(body.subarray(1, 1 + lc), 16);
    detail.push(`APDU: Lc=${lc}, data=${data}, Le=${le(body[1 + lc] ?? 0)}（case 4）`);
  } else {
    anomalies.push(`APDU 长度不合法：Lc=${lc}，其后 ${body.length} 字节`);
    detail.push(`APDU: Lc=${lc}, data=${toHex(body.subarray(1), 16)}（长度不合法）`);
  }
  return name;
}

/** 解 ISO 7816-3 ATR；返回单行摘要，接口字节与历史字节行追加到调用方数组。 */
function decodeAtr(atr: Uint8Array, detail: string[], anomalies: string[]): string {
  if (atr.length < 2) {
    anomalies.push(`ATR 不足 2 字节（实际 ${atr.length} 字节）`);
    detail.push(`ATR 原始字节=${toHex(atr, 16)}`);
    return "ATR 长度不足";
  }
  const ts = byteAt(atr, 0) ?? 0;
  const t0 = byteAt(atr, 1) ?? 0;
  const convention = ts === 0x3b ? "直接约定" : ts === 0x3f ? "反向约定" : "未收录";
  if (convention === "未收录") anomalies.push(`ATR TS=${hex(ts)} 不是 0x3B/0x3F`);
  detail.push(`ATR: TS=${hex(ts)}（${convention}）, T0=${hex(t0)}, K=${t0 & 0x0f} 个历史字节`);
  const cut = (where: string): string => {
    anomalies.push(`ATR 在 ${where} 处被截断`);
    return "ATR 截断";
  };
  let pos = 2;
  let presence = t0;
  let level = 1;
  let protocol = 0;
  while ((presence & 0xf0) !== 0) {
    for (const [mask, label] of ATR_MARKS) {
      if ((presence & mask) === 0) continue;
      if (pos >= atr.length) return cut(`${label}${level}`);
      detail.push(`${label}${level}=${hex(atr[pos++] ?? 0)}`);
    }
    if ((presence & 0x10) === 0) break;
    if (pos >= atr.length) return cut(`TD${level}`);
    const td = atr[pos++] ?? 0;
    detail.push(`TD${level}=${hex(td)}`);
    protocol = td & 0x0f;
    presence = td;
    level += 1;
  }
  const k = t0 & 0x0f;
  const historyEnd = Math.min(pos + k, atr.length);
  const history = atr.subarray(pos, historyEnd);
  if (historyEnd < pos + k)
    anomalies.push(`ATR 历史字节不足：声明 ${k} 字节，实际剩余 ${history.length} 字节`);
  if (k > 0) detail.push(`历史字节=${toHex(history, 16)}`);
  // T≠0 时 ATR 尾部是 TCK 校验字节；T=0 的尾部字节没有把握，按原样呈现。
  if (historyEnd < atr.length) {
    detail.push(
      `${protocol === 0 ? "尾部未解字节" : "TCK"}=${toHex(atr.subarray(historyEnd), 16)}`,
    );
  }
  return `TS ${hex(ts)}（${convention}）, T=${protocol}, ${k} 个历史字节`;
}
