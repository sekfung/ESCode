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

/** 链路层解析出的单个 USB 事务事件。提交与完成各算一条，配对在 `reassemble` 里做。 */
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
  /** 提交事件里声明的期望长度，用于识别短传输。 */
  expectedLength?: number;
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
export const USB_CLASS = {
  audio: 0x01,
  cdc: 0x02,
  hid: 0x03,
  physical: 0x05,
  image: 0x06,
  printer: 0x07,
  massStorage: 0x08,
  hub: 0x09,
  cdcData: 0x0a,
  smartCard: 0x0b,
  video: 0x0e,
  wireless: 0xe0,
  misc: 0xef,
  vendorSpecific: 0xff,
} as const;

/** 接口类到候选协议；未覆盖的类返回空数组（走 `raw` 兜底）。 */
export function protocolsForInterfaceClass(classCode: number): ProtocolName[] {
  switch (classCode) {
    case USB_CLASS.smartCard:
      return ["ccid"];
    case USB_CLASS.hid:
      return ["hid"];
    case USB_CLASS.massStorage:
      return ["msc"];
    default:
      return [];
  }
}

/** 接口类名（用于 `usb_probe` 与 `summary`，避免只看数字）。 */
export function interfaceClassName(classCode: number): string {
  const names: Record<number, string> = {
    [USB_CLASS.audio]: "audio",
    [USB_CLASS.cdc]: "cdc-control",
    [USB_CLASS.hid]: "hid",
    [USB_CLASS.physical]: "physical",
    [USB_CLASS.image]: "still-image",
    [USB_CLASS.printer]: "printer",
    [USB_CLASS.massStorage]: "mass-storage",
    [USB_CLASS.hub]: "hub",
    [USB_CLASS.cdcData]: "cdc-data",
    [USB_CLASS.smartCard]: "smart-card-ccid",
    [USB_CLASS.video]: "video",
    [USB_CLASS.wireless]: "wireless",
    [USB_CLASS.misc]: "miscellaneous",
    [USB_CLASS.vendorSpecific]: "vendor-specific",
  };
  return names[classCode] ?? `class-0x${classCode.toString(16).padStart(2, "0")}`;
}
