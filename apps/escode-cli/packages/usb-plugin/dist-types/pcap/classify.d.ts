/**
 * 从抓包内容里还原设备与接口（docs/specs/usb-agent-tools.md §7「USB 基础层」）。
 *
 * 描述符只在设备枚举时出现在总线上：抓包若从设备已插好之后才开始，就看不到 GET_DESCRIPTOR。
 * 这时只能给出地址（`descriptorSource: "address"`）并把「让用户在抓包期间重新插拔」的建议交给上层，
 * 不能凭 VID:PID 之外的信息猜设备类别。
 */
import { type DeviceSummary, type UsbExchange } from "../types.js";
export interface ClassifyResult {
    devices: Map<string, DeviceSummary>;
    notes: string[];
}
/**
 * 扫描控制传输，得到每个设备（`busDev`）的描述符信息。
 * `addresses` 是所有出现过的设备地址——描述符缺失时它们仍要出现在结果里。
 */
export declare function classifyDevices(exchanges: UsbExchange[], addresses: Iterable<{
    busDev: string;
    bus: number;
    device: number;
}>): ClassifyResult;
//# sourceMappingURL=classify.d.ts.map