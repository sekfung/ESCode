/**
 * USB 抓包分析的数据模型（docs/specs/usb-agent-tools.md §7）。
 *
 * 三层：链路层 `UsbTransfer`（原始 URB）→ `UsbExchange`（配对后的逻辑报文）→ `DecodedMessage`
 * （协议消息，视图与异常的载体）。解码器只依赖这一份类型，运行时与视图层不感知协议细节。
 */
export type UsbDirection = "in" | "out";
export type UsbTransferType = "control" | "bulk" | "interrupt" | "isochronous";
/** 控制传输的 SETUP 阶段（USB 2.0 §9.3）。 */
export interface UsbSetupPacket {
    bmRequestType: number;
    bRequest: number;
    wValue: number;
    wIndex: number;
    wLength: number;
}
/** 链路层解析出的单个 USB 事务事件。提交与完成各算一条，配对在 `pcap/reassemble` 里做。 */
export interface UsbTransfer {
    /** 相对抓包起点的微秒数。 */
    ts: number;
    bus: number;
    device: number;
    /** 含方向位：bit7 置位为 IN。 */
    endpoint: number;
    direction: UsbDirection;
    type: UsbTransferType;
    /** true 为完成事件（数据在 `data` 里），false 为提交事件。 */
    complete: boolean;
    setup?: UsbSetupPacket;
    data: Uint8Array;
    /** 完成事件的状态码，0 表示成功（usbmon 的 status 为负 errno）。 */
    status: number;
    /** URB 标识，用于提交/完成配对（usbmon 的 id / USBPcap 的 irpId）。64 位指针，按十六进制字符串保存。 */
    urbId?: string;
    /**
     * 该事件声明的数据长度：提交侧是请求长度，完成侧是实际长度。
     * 短传输判定靠这两个值的差，不靠 `data.length`（抓包可能被 snaplen 截断）。
     */
    declaredLength?: number;
    /** 数据是否只是零填充（usbmon `flag_data='Z'`）：长度可信、内容不可信。 */
    dataZeroFilled?: boolean;
}
/** 配对后的逻辑报文：控制传输合并 setup + 数据阶段 + 状态，批量/中断合并提交与完成。 */
export interface UsbExchange {
    /** 1 起的编号，`timeline` 与 `message` 视图的索引。 */
    index: number;
    ts: number;
    bus: number;
    device: number;
    /** usbmon 形式的设备地址，如 `1-4`。 */
    busDev: string;
    endpoint: number;
    direction: UsbDirection;
    type: UsbTransferType;
    setup?: UsbSetupPacket;
    data: Uint8Array;
    status: number;
    /** 完成与提交的时间差（微秒，可缺省）。 */
    durationUs?: number;
    /** 提交侧声明的期望长度，用于识别短传输。 */
    expectedLength?: number;
    /** 完成侧的实际长度；与 `expectedLength` 不同即为短传输。 */
    actualLength?: number;
    /** 载荷内容不可信（零填充）时置位，解码器应避免据此下结论。 */
    dataZeroFilled?: boolean;
}
export type ProtocolName = "hid" | "ccid" | "msc" | "raw";
/** 协议解码产出的一条消息；`timeline` 一行，`message` 视图用 `detail` + 正文。 */
export interface DecodedMessage {
    /** 归属的逻辑报文序号（可跨多条，例如链式块合并后的 APDU 交换）。 */
    exchanges: number[];
    ts: number;
    busDev: string;
    direction: UsbDirection;
    /** 协议内类型名，例如 `XfrBlock` / `CSW` / `Input report`。 */
    kind: string;
    /** `timeline` 的单行摘要。 */
    summary: string;
    /** `message` 视图的展开行。 */
    detail: string[];
    /** 计入 `summary` 异常清单的问题（空数组表示无异常）。 */
    anomalies: string[];
    /** 正文；`message` 视图按 `payloadBytes` 截取 16 进制摘要。 */
    payload?: Uint8Array;
}
/** 从抓包描述符里识别出的接口信息。 */
export interface DeviceInterface {
    number: number;
    classCode: number;
    subClassCode: number;
    protocolCode: number;
    /** 由接口类推导的候选协议（`hid` / `ccid` / `msc`）。 */
    protocols: ProtocolName[];
}
/** 从抓包描述符里识别出的设备信息；描述符缺失时只保留地址。 */
export interface DeviceSummary {
    busDev: string;
    bus: number;
    device: number;
    videoId?: number;
    productId?: number;
    manufacturer?: string;
    product?: string;
    serialNumber?: string;
    interfaces: DeviceInterface[];
    /** 描述符来源：抓包内容或仅地址（没有 GET_DESCRIPTOR）。 */
    descriptorSource: "capture" | "address" | "external";
}
export interface DecodeContext {
    /** 同一设备（`busDev`）的逻辑报文，按时间升序。 */
    exchanges: UsbExchange[];
    device: DeviceSummary;
}
export type ProtocolDecoder = (context: DecodeContext) => DecodedMessage[];
/** USB 接口类（USB 2.0 §9.6.3 / 各设备类规范）。 */
export declare const USB_CLASS: {
    readonly audio: 1;
    readonly cdc: 2;
    readonly hid: 3;
    readonly physical: 5;
    readonly image: 6;
    readonly printer: 7;
    readonly massStorage: 8;
    readonly hub: 9;
    readonly cdcData: 10;
    readonly smartCard: 11;
    readonly video: 14;
    readonly wireless: 224;
    readonly misc: 239;
    readonly vendorSpecific: 255;
};
/** 接口类到候选协议；未覆盖的类返回空数组（走 `raw` 兜底）。 */
export declare function protocolsForInterfaceClass(classCode: number): ProtocolName[];
/** 接口类名（用于 `usb_probe` 与 `summary`，避免只看数字）。 */
export declare function interfaceClassName(classCode: number): string;
//# sourceMappingURL=types.d.ts.map