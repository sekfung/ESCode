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
import type { UsbTransfer } from "../types.js";
import type { LinkProblem } from "./usbmon.js";
export declare function isUsbpcapLinkType(linkType: number): boolean;
/** 解一条 USBPcap 记录；返回 undefined 表示该记录不进入分析，原因写进 `problems`。 */
export declare function decodeUsbpcapRecord(data: Uint8Array, recordTsMicros: number, problems: LinkProblem[]): UsbTransfer | undefined;
//# sourceMappingURL=usbpcap.d.ts.map