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
import type { UsbTransfer } from "../types.js";
export declare function isUsbmonLinkType(linkType: number): boolean;
export interface LinkProblem {
    code: "unsupportedFormat" | "decode";
    message: string;
}
/**
 * 解一条 usbmon 记录。返回 undefined 表示这条记录不进入分析（填充包、未知类型），
 * 原因写进 `problems`：静默丢弃与「解出垃圾当真」是两回事。
 */
export declare function decodeUsbmonRecord(linkType: number, data: Uint8Array, recordTsMicros: number, problems: LinkProblem[]): UsbTransfer | undefined;
//# sourceMappingURL=usbmon.d.ts.map