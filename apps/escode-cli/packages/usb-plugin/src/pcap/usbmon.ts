/**
 * usbmon 链路层（LINKTYPE_USB_LINUX=189 / LINKTYPE_USB_LINUX_MMAPPED=220）。
 *
 * 字段布局按 Linux `drivers/usb/mon/mon_bin.c` 的 `struct mon_bin_hdr`（非 packed，64 字节对齐）：
 *   id(8) type(1) xfer_type(1) epnum(1) devnum(1) busnum(2) flag_setup(1) flag_data(1)
 *   ts_sec(8) ts_usec(4) status(4) len_urb(4) len_cap(4) s.setup[8] interval(4) start_frame(4)
 *   xfer_flags(4) ndesc(4)   → 共 64 字节，数据紧跟其后。
 *
 * 189 是 2.6.20 时代的头（PKT_SZ_API0=48）：前 48 字节与 220 完全一致（含偏移 40 的 s.setup），
 * 只是没有 interval/start_frame/xfer_flags/ndesc 四个尾字段。因此两种链路类型共用 setup 偏移，
 * 只有头长度不同。这些常量来自内核源码与 LINKTYPE 注册表，不靠猜。
 */

import { readUint16LE } from "../format.js";
import type { UsbSetupPacket, UsbTransfer, UsbTransferType } from "../types.js";

const HEADER_MMAPPED = 64;
const HEADER_LEGACY = 48;
/** 两种头里 setup 联合体的偏移（`struct mon_bin_hdr` 的 s 字段）。 */
const SETUP_OFFSET = 40;

/** xfer_type 的取值顺序按内核 `xfer_to_pipe[] = {CONTROL, ISO, BULK, INTERRUPT}` 索引进 PIPE_*，
 *  实际存储的是 PIPE 常量（ISO=0, Intr=1, Control=2, Bulk=3），与 `struct mon_bin_hdr` 的注释一致。 */
const XFER_TYPES: Record<number, UsbTransferType> = {
  0: "isochronous",
  1: "interrupt",
  2: "control",
  3: "bulk",
};

export function isUsbmonLinkType(linkType: number): boolean {
  return linkType === 189 || linkType === 220;
}

export interface LinkProblem {
  code: "unsupportedFormat" | "decode";
  message: string;
}

function parseSetup(data: Uint8Array, offset: number): UsbSetupPacket | undefined {
  const bmRequestType = data[offset];
  const bRequest = data[offset + 1];
  const wValue = readUint16LE(data, offset + 2);
  const wIndex = readUint16LE(data, offset + 4);
  const wLength = readUint16LE(data, offset + 6);
  if (
    bmRequestType === undefined ||
    bRequest === undefined ||
    wValue === undefined ||
    wIndex === undefined ||
    wLength === undefined
  ) {
    return undefined;
  }
  return { bmRequestType, bRequest, wValue, wIndex, wLength };
}

/**
 * 解一条 usbmon 记录。返回 undefined 表示这条记录不进入分析（填充包、未知类型），
 * 原因写进 `problems`：静默丢弃与「解出垃圾当真」是两回事。
 */
export function decodeUsbmonRecord(
  linkType: number,
  data: Uint8Array,
  recordTsMicros: number,
  problems: LinkProblem[],
): UsbTransfer | undefined {
  const headerSize = linkType === 220 ? HEADER_MMAPPED : HEADER_LEGACY;
  if (data.length < SETUP_OFFSET + 8) {
    problems.push({ code: "decode", message: `usbmon 记录长度不足（${data.length} < ${headerSize}）` });
    return undefined;
  }
  const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
  const typeTag = String.fromCharCode(data[8] ?? 0);
  if (typeTag === "@") return undefined; // 填充包
  const xferType = XFER_TYPES[data[9] ?? 0];
  if (!xferType) {
    problems.push({ code: "decode", message: `未知的 usbmon 传输类型 ${data[9] ?? 0}` });
    return undefined;
  }
  if (typeTag !== "S" && typeTag !== "C" && typeTag !== "E") {
    problems.push({ code: "decode", message: `未知的 usbmon 事件类型 ${JSON.stringify(typeTag)}` });
    return undefined;
  }
  const epnum = data[10] ?? 0;
  const flagSetup = data[14] ?? 0;
  const flagData = data[15] ?? 0;
  // 头部自带的事件时间戳比容器记录时间戳更贴近事件本身，优先用它。
  const headerTsMicros = Number(view.getBigInt64(16, true)) * 1_000_000 + view.getInt32(24, true);
  const status = view.getInt32(28, true);
  const lenUrb = view.getUint32(32, true);
  const lenCap = view.getUint32(36, true);

  const setup = xferType === "control" && flagSetup !== "-".charCodeAt(0)
    ? parseSetup(data, SETUP_OFFSET)
    : undefined;

  // flag_data：0/'D' 表示缓冲区里有真实数据；'Z' 是零填充（长度可信、内容不可信）；
  // '<'（IN 提交）与 '>'（OUT 完成）表示该事件不带数据。
  const hasData = flagData === 0 || flagData === "D".charCodeAt(0);
  const zeroFilled = flagData === "Z".charCodeAt(0);
  const available = Math.max(0, data.length - headerSize);
  if (lenCap > available) {
    problems.push({
      code: "decode",
      message: `usbmon 记录声明 ${lenCap} 字节数据但只有 ${available} 字节（抓包被 snaplen 截断）`,
    });
  }
  const payloadLength = Math.min(lenCap, available);

  return {
    ts: headerTsMicros > 0 ? headerTsMicros : recordTsMicros,
    bus: readUint16LE(data, 12) ?? 0,
    device: data[11] ?? 0,
    endpoint: epnum,
    direction: (epnum & 0x80) !== 0 ? "in" : "out",
    type: xferType,
    complete: typeTag !== "S",
    ...(setup ? { setup } : {}),
    data: hasData || zeroFilled
      ? data.subarray(headerSize, headerSize + payloadLength)
      : new Uint8Array(0),
    status,
    urbId: `0x${view.getBigUint64(0, true).toString(16)}`,
    declaredLength: lenUrb,
    ...(zeroFilled ? { dataZeroFilled: true } : {}),
  };
}
