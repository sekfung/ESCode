import { type DecodeContext, type DecodedMessage } from "../types.js";
export interface HidField {
    /** bit 偏移（相对该 reportId 的报告正文起点，含 reportId 字节后）。 */
    bitOffset: number;
    bitSize: number;
    usagePage: number;
    usages: number[];
    /** 变量字段的有符号解释（logical minimum 为负时为 true）。 */
    signed: boolean;
    flags: {
        constant: boolean;
        variable: boolean;
        relative: boolean;
    };
}
export interface HidReportLayout {
    reportId: number;
    /** 最近一次生成字段的主条目参数（布局只有 reportId 一维，不区分输入/输出流）。 */
    reportSize: number;
    reportCount: number;
    fields: HidField[];
}
/** 解析产物；`parseHidReportDescriptor` 只暴露 `layouts`。 */
export interface HidDescriptorParseResult {
    layouts: HidReportLayout[];
    anomalies: string[];
    unknownItems: string[];
    /** 数组字段元素位宽（公开 HidField 没有这一维）与每个 reportId 的输入流位长。 */
    arrayElements: {
        reportId: number;
        bitOffset: number;
        elementBits: number;
    }[];
    inputBits: {
        reportId: number;
        bits: number;
    }[];
}
export declare function parseHidReportDescriptorDetailed(descriptor: Uint8Array): HidDescriptorParseResult;
/** 报告描述符 → 报告布局（导出以便单测与 `usb_probe` 复用）。 */
export declare function parseHidReportDescriptor(descriptor: Uint8Array): HidReportLayout[];
/** 只对确认是 HID 的设备产出消息：接口类 0x03，或抓包里出现过报告描述符。 */
export declare function decodeHidMessages(context: DecodeContext): DecodedMessage[];
//# sourceMappingURL=hid.d.ts.map