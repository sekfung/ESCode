/**
 * 分析编排与视图（docs/specs/usb-agent-tools.md §3.4 / §8）。
 *
 * 链路：容器 → 链路层 → 逻辑报文 → 按设备分组 → 协议消息 → 视图裁剪。
 * 所有视图都有硬上限：summary 恒定小体积，timeline 分页，message 单条——抓包动辄数万条，
 * 任何「把结果全量交给模型」的实现都会把上下文烧光。
 */
import { type ProtocolName, type UsbDirection } from "./types.js";
export type AnalysisView = "summary" | "timeline" | "message";
export interface AnalysisFilter {
    busDev?: string;
    endpoint?: number;
    direction?: UsbDirection;
    onlyErrors?: boolean;
}
export interface AnalysisOptions {
    protocol: ProtocolName | "auto";
    view: AnalysisView;
    filter?: AnalysisFilter;
    index?: number;
    limit?: number;
    cursor?: string;
    payloadBytes?: number;
}
export interface AnalysisOutput {
    /** 给模型看的紧凑文本（一行一条报文）。 */
    text: string;
    /** 结构化元数据：计数、游标、截断标志；体积固定，不含逐条报文。 */
    meta: Record<string, unknown>;
}
export declare function analyzeCapture(data: Uint8Array, options: AnalysisOptions): AnalysisOutput;
//# sourceMappingURL=analysis.d.ts.map