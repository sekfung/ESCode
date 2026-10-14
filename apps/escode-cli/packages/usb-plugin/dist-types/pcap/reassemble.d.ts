/**
 * 提交/完成配对：把链路层事件合成逻辑报文（docs/specs/usb-agent-tools.md §7）。
 *
 * 规则来自 usbmon 与 USBPcap 的实际行为：
 * - 控制传输：提交事件带 SETUP，数据阶段与状态在完成事件里；控制 OUT 的数据可能在提交侧。
 * - 批量/中断 OUT：数据在**提交**事件（主机发送时抓）；IN 的数据在**完成**事件。
 *   因此载荷取「完成侧非空则用完成侧，否则用提交侧」，而不是固定取一侧。
 * - 等时：不做数据重组，一次完成事件算一条报文。
 */
import type { UsbExchange, UsbTransfer } from "../types.js";
export interface ReassemblyResult {
    exchanges: UsbExchange[];
    warnings: string[];
}
export declare function toExchanges(transfers: UsbTransfer[]): ReassemblyResult;
//# sourceMappingURL=reassemble.d.ts.map