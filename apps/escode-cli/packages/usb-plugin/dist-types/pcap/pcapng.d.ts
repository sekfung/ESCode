/**
 * pcap / pcapng 容器读取（docs/specs/usb-agent-tools.md §7）。
 *
 * 只做容器层：把每个数据包连同它所属接口的链路类型与时间戳交出去，链路层解析在 usbmon.ts / usbpcap.ts。
 * 规则：**不猜格式**——容器或链路类型不认识就报 `unsupportedFormat`；文件被截断（抓包中途被杀）时
 * 读到坏块即停，返回已解析部分并置 `truncated`，绝不把截断当成功。
 */
export declare class CaptureFormatError extends Error {
    readonly code: "unsupportedFormat" | "decode";
    constructor(code: "unsupportedFormat" | "decode", message: string);
}
export interface CaptureInterface {
    id: number;
    linkType: number;
    snaplen: number;
    name?: string;
    /** 时间戳单位：每秒多少个 tick（pcapng 的 if_tsresol）。 */
    ticksPerSecond: number;
}
export interface CaptureRecord {
    interfaceId: number;
    /** 该包所属接口的链路类型；调用方据此选择链路层解析器，不必自己查接口表。 */
    linkType: number;
    tsMicros: number;
    data: Uint8Array;
    originalLength: number;
}
export interface ReadCaptureResult {
    format: "pcap" | "pcapng";
    interfaces: CaptureInterface[];
    records: number;
    /** 文件在正常情况下提前结束（截断、坏块），只保证已产出的记录可用。 */
    truncated: boolean;
    warnings: string[];
}
/** 按魔数选择容器；未知容器抛 `unsupportedFormat`（不猜）。 */
export declare function readCaptureData(data: Uint8Array, onRecord: (record: CaptureRecord) => void): ReadCaptureResult;
//# sourceMappingURL=pcapng.d.ts.map