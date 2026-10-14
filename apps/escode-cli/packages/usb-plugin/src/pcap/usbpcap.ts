/**
 * USBPcap 链路层（LINKTYPE_USBPCAP=249）。
 *
 * 伪头按 USBPcap 驱动头文件 `USBPCAP_BUFFER_PACKET_HEADER`（`#pragma pack(1)`，小端）：
 *   headerLen(2) irpId(8) status(4) function(2) info(1) bus(2) device(2) endpoint(1) transfer(1)
 *   dataLength(4)  → 27 字节；控制传输再追加 stage(1)。数据区起点固定为 `headerLen`。
 *
 * `info` 只有 bit0 有定义（`USBPCAP_INFO_PDO_TO_FDO`）：**0 表示请求下发（提交），1 表示完成**——
 * 注意这与字段名的字面直觉相反，按官方捕获格式文档实现。
 * 控制传输自 USBPcap 1.5.0 起拆成两个包：SETUP 包（载荷 = 8 字节 SETUP + 可选 DATA OUT）与
 * COMPLETE 包（载荷 = DATA IN 或空）。
 */

import type { UsbSetupPacket, UsbTransfer, UsbTransferType } from "../types.js";
import type { LinkProblem } from "./usbmon.js";

/** transfer 字段取值（USBPCAP_TRANSFER_*）。 */
const TRANSFER_TYPES: Record<number, UsbTransferType> = {
  0: "isochronous",
  1: "interrupt",
  2: "control",
  3: "bulk",
};

const BASE_HEADER_BYTES = 27;
const SETUP_STAGE = 0;

export function isUsbpcapLinkType(linkType: number): boolean {
  return linkType === 249;
}

function parseSetup(payload: Uint8Array): UsbSetupPacket | undefined {
  if (payload.length < 8) return undefined;
  const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
  return {
    bmRequestType: payload[0] ?? 0,
    bRequest: payload[1] ?? 0,
    wValue: view.getUint16(2, true),
    wIndex: view.getUint16(4, true),
    wLength: view.getUint16(6, true),
  };
}

/** 解一条 USBPcap 记录；返回 undefined 表示该记录不进入分析，原因写进 `problems`。 */
export function decodeUsbpcapRecord(
  data: Uint8Array,
  recordTsMicros: number,
  problems: LinkProblem[],
): UsbTransfer | undefined {
  if (data.length < BASE_HEADER_BYTES) {
    problems.push({ code: "decode", message: `USBPcap 记录长度不足（${data.length} < 27）` });
    return undefined;
  }
  const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
  const headerLen = view.getUint16(0, true);
  const info = data[16] ?? 0;
  const endpoint = data[21] ?? 0;
  const transferTag = data[22] ?? 0;
  const dataLength = view.getUint32(23, true);

  // URB id 是 64 位指针，超过 2^53 时 Number 会失真，因此统一以十六进制字符串保存。
  const urbId = `0x${view.getBigUint64(2, true).toString(16)}`;

  if (headerLen < BASE_HEADER_BYTES || headerLen > data.length) {
    problems.push({ code: "decode", message: `USBPcap 头部长度非法（${headerLen}）` });
    return undefined;
  }
  const transferType = TRANSFER_TYPES[transferTag];
  if (!transferType) {
    // 0xFE = IRP_INFO（元信息）、0xFF = UNKNOWN：不是 USB 事务，直接跳过而不计入异常。
    return undefined;
  }
  const payloadEnd = headerLen + dataLength;
  if (payloadEnd > data.length) {
    problems.push({
      code: "decode",
      message: `USBPcap 记录声明 ${dataLength} 字节数据但只有 ${data.length - headerLen} 字节`,
    });
  }
  const payload = data.subarray(headerLen, Math.min(payloadEnd, data.length));
  const stage = transferType === "control" && data.length > BASE_HEADER_BYTES ? data[BASE_HEADER_BYTES] : undefined;

  let setup: UsbSetupPacket | undefined;
  let body = payload;
  if (transferType === "control" && stage === SETUP_STAGE) {
    setup = parseSetup(payload);
    body = payload.subarray(Math.min(8, payload.length));
  }

  const complete = (info & 0x01) !== 0;
  return {
    ts: recordTsMicros,
    bus: view.getUint16(17, true),
    device: view.getUint16(19, true),
    endpoint,
    direction: (endpoint & 0x80) !== 0 ? "in" : "out",
    type: transferType,
    complete,
    ...(setup ? { setup } : {}),
    data: body,
    status: view.getUint32(10, true),
    urbId,
    // USBPcap 头里没有 URB 请求长度，只有本次捕获到的数据长度：
    // 因此 Windows 抓包不做短传输判定（usbmon 才有 len_urb 可比）。
    declaredLength: dataLength,
  };
}
