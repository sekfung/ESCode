/**
 * MSC / SCSI（USB 大容量存储，Bulk-Only Transport）解码：docs/specs/usb-agent-tools.md §7。
 * 同一设备的 CBW（31 字节）→ 数据阶段 → CSW（13 字节）串成一条逻辑命令；接口类不可信
 * （不少 U 盘把类报成 0xFF），只按 CBW/CSW 签名识别，确认不了是 BOT 的报文一律跳过。
 */
import { byteAt, hex, readUint16BE, readUint32BE, readUint32LE, toAscii, toHex } from "../format.js";
import type { DecodeContext, DecodedMessage, UsbExchange } from "../types.js";

const CBW_SIGNATURE = 0x43425355;
const CSW_SIGNATURE = 0x53425355;
const CBW_LENGTH = 31;
const CSW_LENGTH = 13;
/** IN 响应只按定长头部解码（INQUIRY 36、固定格式 sense 18），不把整包留在内存里。 */
const DATA_HEAD_LIMIT = 64;
/** CDB 操作码表：只收录有把握的，其余给 `unknown(0xNN)` 并计异常，不猜。 */
const CDB_NAMES: Record<number, string> = {
  0x00: "TEST UNIT READY", 0x03: "REQUEST SENSE", 0x04: "FORMAT UNIT", 0x12: "INQUIRY",
  0x1a: "MODE SENSE(6)", 0x1b: "START STOP UNIT", 0x1e: "PREVENT ALLOW MEDIUM REMOVAL",
  0x23: "READ FORMAT CAPACITIES", 0x25: "READ CAPACITY(10)", 0x28: "READ(10)", 0x2a: "WRITE(10)",
  0x2f: "VERIFY(10)", 0x35: "SYNCHRONIZE CACHE(10)", 0x5a: "MODE SENSE(10)", 0x88: "READ(16)",
  0x8a: "WRITE(16)", 0xa0: "REPORT LUNS", 0xa8: "READ(12)", 0xaa: "WRITE(12)",
};
/** 读写类 CDB 的字段布局（SBC）：`[CDB 字节数, LBA 字节数, 长度偏移, 长度字节数]`，字段均大端。 */
const BLOCK_LAYOUTS: Record<number, [number, number, number, number]> = {
  0x28: [10, 4, 7, 2], 0x2a: [10, 4, 7, 2], 0x2f: [10, 4, 7, 2], 0x35: [10, 4, 7, 2],
  0xa8: [10, 4, 6, 4], 0xaa: [10, 4, 6, 4], 0x88: [14, 8, 10, 4], 0x8a: [14, 8, 10, 4],
};
/** SCSI 外设类型（SPC 常用子集），未知按十六进制。 */
const PERIPHERAL_TYPES: Record<number, string> = {
  0x00: "直接存取设备", 0x01: "顺序存取设备", 0x02: "打印机", 0x04: "一次写入设备",
  0x05: "CD/DVD", 0x07: "光磁设备", 0x0e: "简化块设备", 0x1f: "未知或无设备",
};
const SENSE_KEYS: Record<number, string> = {
  0x0: "NO SENSE", 0x1: "RECOVERED ERROR", 0x2: "NOT READY", 0x3: "MEDIUM ERROR",
  0x4: "HARDWARE ERROR", 0x5: "ILLEGAL REQUEST", 0x6: "UNIT ATTENTION", 0x7: "DATA PROTECT",
  0xb: "ABORTED COMMAND",
};
/** ASC/ASCQ 常见组合，键为 `(ASC << 8) | ASCQ`；表外给十六进制并标注「未收录」。 */
const ASC_ASCQ_NAMES: Record<number, string> = {
  0x0000: "无附加信息", 0x0401: "正在就绪", 0x0402: "需要初始化", 0x1100: "未恢复读错误",
  0x2100: "LBA 超出范围", 0x2400: "CDB 字段非法", 0x2700: "写保护", 0x2800: "介质更换",
  0x2900: "上电复位", 0x3a00: "无介质",
};
interface Cbw { tag: number; transferLength: number; inData: boolean; lun: number; cbLength: number; cdb: Uint8Array }
interface Csw { tag: number; residue: number; status: number }
/** `short` 进摘要/异常，`lines` 进 detail。 */
interface SenseResult { short: string; lines: string[] }
/** 一条逻辑命令汇聚期间的中间状态。 */
interface CommandRecord {
  ts: number; busDev: string; direction: "in" | "out"; exchanges: number[];
  cbw?: Cbw; cbwRaw?: Uint8Array; cbwTruncatedAt?: number;
  dataBytes: number; dataExchanges: number; dataHead: number[];
  csw?: Csw; cswRaw?: Uint8Array; cswTruncatedAt?: number;
  anomalies: string[];
}
interface CdbInfo { name: string; summary: string; details: string[]; anomalies: string[]; opcode: number | undefined }

/** 解 USB 大容量存储（BOT）：入参是该设备（同一 busDev）的全部逻辑报文，按时间升序。 */
export function decodeMscMessages(context: DecodeContext): DecodedMessage[] {
  const records: CommandRecord[] = [];
  let pending: CommandRecord | undefined;
  for (const exchange of context.exchanges) {
    // BOT 只走 bulk 端点；控制/中断报文上的同名签名不是命令。
    if (exchange.type !== "bulk") continue;
    const signature = readUint32LE(exchange.data, 0);
    if (signature === CBW_SIGNATURE) {
      if (pending !== undefined) records.push(closePending(pending, "CBW 之后没有收到 CSW（命令未完成）"));
      pending = undefined;
      const record = createRecord(exchange);
      if (exchange.data.length < CBW_LENGTH) {
        record.cbwRaw = exchange.data;
        record.cbwTruncatedAt = exchange.data.length;
        record.anomalies.push(`CBW 长度不足：${exchange.data.length} 字节（需要 ${CBW_LENGTH} 字节）`);
        records.push(record);
      } else {
        record.cbw = parseCbw(exchange.data);
        record.cbwRaw = exchange.data;
        pending = record;
      }
      continue;
    }
    if (signature === CSW_SIGNATURE) {
      if (pending === undefined) {
        // 没有 CBW 的 CSW：孤立响应，或截断 CBW 之后的残留，单独成一条。
        const record = createRecord(exchange);
        record.cswRaw = exchange.data;
        if (exchange.data.length < CSW_LENGTH) {
          record.cswTruncatedAt = exchange.data.length;
          record.anomalies.push(`CSW 长度不足：${exchange.data.length} 字节（需要 ${CSW_LENGTH} 字节）`);
        } else {
          record.csw = parseCsw(exchange.data);
          record.anomalies.push("CSW 没有匹配的 CBW（孤立响应）");
        }
        records.push(record);
        continue;
      }
      pending.exchanges.push(exchange.index);
      pending.cswRaw = exchange.data;
      if (exchange.data.length < CSW_LENGTH) {
        pending.cswTruncatedAt = exchange.data.length;
        pending.anomalies.push(`CSW 长度不足：${exchange.data.length} 字节（需要 ${CSW_LENGTH} 字节）`);
      } else {
        pending.csw = parseCsw(exchange.data);
      }
      records.push(pending);
      pending = undefined;
      continue;
    }
    // 其余 bulk 报文归属数据阶段（含零长度 ZLP），只累计字节数与前 64 字节头部。
    if (pending !== undefined) {
      pending.dataBytes += exchange.data.length;
      pending.dataExchanges += 1;
      pending.exchanges.push(exchange.index);
      for (const byte of exchange.data) {
        if (pending.dataHead.length >= DATA_HEAD_LIMIT) break;
        pending.dataHead.push(byte);
      }
    }
  }
  if (pending !== undefined) records.push(closePending(pending, "CBW 之后没有收到 CSW（命令未完成）"));

  // SCSI 约定：失败命令的 sense 由其后最近的 REQUEST SENSE 带回（连续失败只归最近一个）。
  const ownSense = new Map<number, SenseResult>();
  records.forEach((record, position) => {
    if (record.cbw === undefined || byteAt(record.cbw.cdb, 0) !== 0x03) return;
    const sense = parseSense(headBytes(record));
    if (sense !== undefined) ownSense.set(position, sense);
  });
  const failureSense = new Map<number, SenseResult>();
  let lastFailed = -1;
  records.forEach((record, position) => {
    const sense = ownSense.get(position);
    if (sense !== undefined) {
      if (lastFailed >= 0) failureSense.set(lastFailed, sense);
      lastFailed = -1;
    } else if (record.csw !== undefined && record.csw.status !== 0) {
      lastFailed = position;
    }
  });
  return records.map((record, position) =>
    buildMessage(record, ownSense.get(position), failureSense.get(position), ownSense.size > 0),
  );
}
function closePending(record: CommandRecord, anomaly: string): CommandRecord {
  record.anomalies.push(anomaly);
  return record;
}
function createRecord(exchange: UsbExchange): CommandRecord {
  return {
    ts: exchange.ts, busDev: exchange.busDev, direction: exchange.direction, exchanges: [exchange.index],
    dataBytes: 0, dataExchanges: 0, dataHead: [], anomalies: [],
  };
}
function headBytes(record: CommandRecord): Uint8Array {
  return Uint8Array.from(record.dataHead);
}
/** 仅在长度 ≥ 31 时调用。 */
function parseCbw(data: Uint8Array): Cbw | undefined {
  const tag = readUint32LE(data, 4);
  const transferLength = readUint32LE(data, 8);
  const flags = byteAt(data, 12);
  const lun = byteAt(data, 13);
  const rawCbLength = byteAt(data, 14);
  if (tag === undefined || transferLength === undefined || flags === undefined) return undefined;
  if (lun === undefined || rawCbLength === undefined) return undefined;
  const cbLength = rawCbLength & 0x1f;
  return {
    tag, transferLength, inData: (flags & 0x80) !== 0, lun: lun & 0x0f, cbLength,
    cdb: data.subarray(15, 15 + Math.min(cbLength, 16)),
  };
}
/** 仅在长度 ≥ 13 时调用。 */
function parseCsw(data: Uint8Array): Csw | undefined {
  const tag = readUint32LE(data, 4);
  const residue = readUint32LE(data, 8);
  const status = byteAt(data, 12);
  if (tag === undefined || residue === undefined || status === undefined) return undefined;
  return { tag, residue, status };
}
function buildMessage(
  record: CommandRecord, ownSense: SenseResult | undefined,
  assignedSense: SenseResult | undefined, hasRequestSense: boolean,
): DecodedMessage {
  if (record.cbw === undefined) return buildUnpairedMessage(record);
  const cbw = record.cbw;
  const cdb = describeCdb(cbw.cdb);
  const details = [
    `CBW: tag=${hex(cbw.tag, 8)} LUN=${cbw.lun} 方向=${cbw.inData ? "IN" : "OUT"} 声明长度=${cbw.transferLength} 字节`,
    ...cdb.details,
  ];
  const anomalies = [...record.anomalies, ...cdb.anomalies];
  if (cbw.cbLength > 16) anomalies.push(`bCBWCBLength=${cbw.cbLength} 超过 16（CDB 已按 16 字节截断）`);
  if (record.dataExchanges > 0) details.push(`数据阶段: ${record.dataBytes} 字节（${record.dataExchanges} 条报文）`);
  if (cdb.opcode === 0x12 && record.dataBytes > 0) details.push(...describeInquiry(headBytes(record)));
  anomalies.push(...transferAnomalies(cbw, record.dataBytes, record.csw));
  let summary = cdb.summary;
  if (record.dataBytes > 0) summary += ` · 数据 ${record.dataBytes} 字节`;
  if (record.csw !== undefined) {
    const csw = record.csw;
    details.push(`CSW: tag=${hex(csw.tag, 8)} 残差=${csw.residue} 状态=${hex(csw.status)} ${cswStatusName(csw.status)}`);
    if (csw.tag !== cbw.tag) anomalies.push(`CSW tag=${hex(csw.tag, 8)} 与 CBW tag=${hex(cbw.tag, 8)} 不一致`);
    if (csw.status !== 0) {
      const failure = `CSW 状态 ${hex(csw.status)}（${cswStatusName(csw.status)}）`;
      if (assignedSense !== undefined) anomalies.push(`${failure}：${assignedSense.short}`);
      else if (hasRequestSense) anomalies.push(`${failure}，但其后没有 REQUEST SENSE，无法确认该命令的 sense`);
      else anomalies.push(`${failure}，抓包内没有 REQUEST SENSE；建议让用户在复现时抓包以取到 sense key`);
    }
    summary += ` · ${cswSummary(csw)}`;
  } else if (record.cswTruncatedAt !== undefined) {
    details.push(`CSW 不完整: ${record.cswTruncatedAt} 字节（需要 ${CSW_LENGTH} 字节）`);
    summary += " · CSW 不完整";
  } else {
    summary += " · 无 CSW";
  }
  const sense = ownSense ?? assignedSense;
  if (sense !== undefined) {
    details.push(...sense.lines);
    summary += ` · ${sense.short}`;
  } else if (cdb.opcode === 0x03 && record.dataBytes > 0) {
    const code = byteAt(headBytes(record), 0);
    details.push(`REQUEST SENSE 响应未按固定格式解出（响应码 ${code === undefined ? "缺失" : hex(code)}，仅收录 0x70/0x71）`);
  }
  return toMessage(record, cdb.name, summary, details, anomalies, cbw.cdb);
}
/** 缺 CBW 或 CSW 的残缺记录；能确认多少给多少，不补猜。 */
function buildUnpairedMessage(record: CommandRecord): DecodedMessage {
  const details: string[] = [];
  if (record.csw !== undefined) {
    const csw = record.csw;
    details.push(`CSW: tag=${hex(csw.tag, 8)} 残差=${csw.residue} 状态=${hex(csw.status)} ${cswStatusName(csw.status)}`);
    return toMessage(record, "CSW", `CSW 孤立响应 tag=${hex(csw.tag, 8)} · ${cswSummary(csw)}`, details, record.anomalies, record.cswRaw);
  }
  const truncatedCbw = record.cbwTruncatedAt !== undefined;
  const size = (truncatedCbw ? record.cbwTruncatedAt : record.cswTruncatedAt) ?? 0;
  const name = truncatedCbw ? "CBW" : "CSW";
  const raw = truncatedCbw ? record.cbwRaw : record.cswRaw;
  if (raw !== undefined) details.push(`原始字节(${raw.length}): ${toHex(raw, 32)}`);
  const needed = truncatedCbw ? CBW_LENGTH : CSW_LENGTH;
  return toMessage(record, `${name} 截断`, `${name} 截断（${size} 字节，需要 ${needed} 字节）`, details, record.anomalies, raw);
}
function describeCdb(cdb: Uint8Array): CdbInfo {
  const details: string[] = [];
  const anomalies: string[] = [];
  const opcode = byteAt(cdb, 0);
  if (opcode === undefined) {
    anomalies.push("bCBWCBLength=0，CBW 里没有 CDB");
    return { name: "无 CDB", summary: "无 CDB", details, anomalies, opcode: undefined };
  }
  details.push(`CDB(${cdb.length}): ${toHex(cdb)}`);
  const known = CDB_NAMES[opcode];
  const name = known ?? `unknown(${hex(opcode)})`;
  if (known === undefined) anomalies.push(`未知 CDB 操作码 ${hex(opcode)}（未收录，不做猜测）`);
  let summary = name;
  const layout = known === undefined ? undefined : BLOCK_LAYOUTS[opcode];
  if (layout !== undefined) {
    const [need, lbaBytes, lengthOffset, lengthBytes] = layout;
    const lba = cdb.length < need ? undefined : readHexField(cdb, 2, lbaBytes);
    const length = cdb.length < need ? undefined
      : lengthBytes === 2 ? readUint16BE(cdb, lengthOffset) : readUint32BE(cdb, lengthOffset);
    if (lba === undefined || length === undefined) {
      anomalies.push(`CDB 长度不足：${name} 需要 ${need} 字节，实际 ${cdb.length} 字节，无法解出 LBA/长度`);
    } else {
      summary = `${name} LBA=0x${lba} len=${length} 块`;
    }
  }
  return { name, summary, details, anomalies, opcode };
}
/** 实际字节数与 CBW 声明长度、CSW 残差互相比对，只报不一致。 */
function transferAnomalies(cbw: Cbw, actual: number, csw: Csw | undefined): string[] {
  const declared = cbw.transferLength;
  const delta = declared - actual;
  const kind = delta > 0 ? "短传输" : "多余传输";
  const mismatch = `${kind}：数据阶段 ${actual} 字节，CBW 声明 ${declared} 字节`;
  if (csw === undefined) return delta === 0 ? [] : [mismatch];
  if (csw.residue === 0) return delta === 0 ? [] : [`${mismatch}（CSW 残差为 0，两者不符）`];
  if (csw.residue === delta) return [`${mismatch}，CSW 残差 ${csw.residue} 字节`];
  return [
    `CSW 残差 ${csw.residue} 字节与 CBW 声明长度/实际数据（${declared}/${actual}）不一致`,
    ...(delta === 0 ? [] : [mismatch]),
  ];
}
/** 固定格式 sense data（0x70/0x71）：sense key 在字节 2 低 4 位，ASC/ASCQ 在字节 12-13。 */
function parseSense(data: Uint8Array): SenseResult | undefined {
  const responseCode = byteAt(data, 0);
  const keyByte = byteAt(data, 2);
  if (keyByte === undefined || (responseCode !== 0x70 && responseCode !== 0x71)) return undefined;
  const senseKey = keyByte & 0x0f;
  const keyName = SENSE_KEYS[senseKey] ?? `未知(${hex(senseKey, 1)})`;
  const lines = [`sense: 响应码 ${hex(responseCode)}（${responseCode === 0x70 ? "当前错误" : "延迟错误"}） sense key=${keyName}(${hex(senseKey, 1)})`];
  const asc = byteAt(data, 12);
  const ascq = byteAt(data, 13);
  if (asc === undefined || ascq === undefined) {
    lines.push(`sense: 数据仅 ${data.length} 字节，缺少 ASC/ASCQ`);
    return { short: keyName, lines };
  }
  const name = ASC_ASCQ_NAMES[(asc << 8) | ascq];
  const ascText = name === undefined ? `${hex(asc)}/${hex(ascq)}（未收录）` : `${hex(asc)}/${hex(ascq)} ${name}`;
  lines.push(`sense: ASC/ASCQ=${ascText}`);
  return { short: `${keyName} ${ascText}`, lines };
}
/** INQUIRY 响应：字节 0 外设类型，8-15 vendor，16-31 product，32-35 revision（ASCII，去尾部空格）。 */
function describeInquiry(data: Uint8Array): string[] {
  const typeByte = byteAt(data, 0);
  if (typeByte === undefined) return ["INQUIRY 响应为空"];
  const code = typeByte & 0x1f;
  const lines = [`INQUIRY 外设类型: ${PERIPHERAL_TYPES[code] ?? `未知(${hex(code)})`} (${hex(code)})`];
  const vendor = asciiField(data, 8, 16);
  const product = asciiField(data, 16, 32);
  const revision = asciiField(data, 32, 36);
  if (vendor) lines.push(`INQUIRY vendor: ${vendor}`);
  if (product) lines.push(`INQUIRY product: ${product}`);
  if (revision) lines.push(`INQUIRY revision: ${revision}`);
  if (data.length < 36) lines.push(`INQUIRY 响应仅 ${data.length} 字节，vendor/product/revision 可能不完整`);
  return lines;
}

/** 取 ASCII 字段；不足整段返回 undefined。`toAscii` 把不可打印字节显示为 `.`，连同尾部空格一起去掉。 */
function asciiField(data: Uint8Array, start: number, end: number): string | undefined {
  if (end > data.length) return undefined;
  return toAscii(data.subarray(start, end)).replace(/[ .]+$/, "");
}
/** LBA 可能是 64 位，逐字节转十六进制再拼，避免 number 精度丢失；去掉前导零。 */
function readHexField(data: Uint8Array, offset: number, bytes: number): string | undefined {
  let text = "";
  for (let index = 0; index < bytes; index += 1) {
    const byte = byteAt(data, offset + index);
    if (byte === undefined) return undefined;
    text += byte.toString(16).padStart(2, "0");
  }
  return text.replace(/^0+(?=.)/, "").toUpperCase();
}
function cswStatusName(status: number): string {
  if (status === 0x00) return "成功";
  if (status === 0x01) return "失败";
  if (status === 0x02) return "phase error";
  return `未知(${hex(status)})`;
}
function cswSummary(csw: Csw): string {
  return csw.status === 0 ? "CSW 成功" : `CSW ${cswStatusName(csw.status)}(${hex(csw.status)})`;
}
function toMessage(
  record: CommandRecord, kind: string, summary: string, detail: string[],
  anomalies: string[], payload?: Uint8Array,
): DecodedMessage {
  return {
    exchanges: record.exchanges, ts: record.ts, busDev: record.busDev, direction: record.direction,
    kind, summary, detail, anomalies, payload,
  };
}
