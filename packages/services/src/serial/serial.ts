import type { Event } from "@zcode/rpc";
import { ServiceChannels } from "@zcode/shared";
import { createServiceDescriptor } from "../descriptors.js";

/** 写入来源；rx 方向固定为 "user"（表示外部设备），不代表人工操作。 */
export type SerialSource = "user" | "agent";
export type SerialDirection = "rx" | "tx";

export interface SerialPortInfo {
  path: string;
  manufacturer?: string;
  serialNumber?: string;
  vendorId?: string;
  productId?: string;
}

export interface SerialConfig {
  baudRate: number;
  dataBits: 5 | 6 | 7 | 8;
  parity: "none" | "even" | "odd" | "mark" | "space";
  stopBits: 1 | 1.5 | 2;
  rtscts: boolean;
  autoReconnect: boolean;
}

export interface SerialChunk {
  seq: number;
  /** epoch ms，Host 收到数据或写入完成时记录。 */
  at: number;
  direction: SerialDirection;
  source: SerialSource;
  bytes: Uint8Array;
}

export type SerialErrorCode =
  | "busy"
  | "denied"
  | "notFound"
  | "invalidConfig"
  | "invalidInput"
  | "nativeUnavailable"
  | "notOpen"
  | "io";

export type SerialState = "closed" | "opening" | "open" | "closing" | "disconnected" | "error";

export interface SerialStatus {
  state: SerialState;
  path?: string;
  config?: SerialConfig;
  error?: { code: SerialErrorCode; message: string };
}

export interface SerialStats {
  rxBytes: number;
  txBytes: number;
}

export interface SerialSnapshot {
  status: SerialStatus;
  chunks: SerialChunk[];
  /** 快照包含的最大 seq；订阅期间收到 seq ≤ 该值的 onData 需丢弃。 */
  seq: number;
  stats: SerialStats;
}

export const SERIAL_WRITE_LIMIT_BYTES = 64 * 1024;

export class SerialError extends Error {
  constructor(
    readonly code: SerialErrorCode,
    message: string,
  ) {
    super(message);
    this.name = "SerialError";
  }
}

export interface ISerialService {
  list(): Promise<SerialPortInfo[]>;
  open(params: { path: string; config: SerialConfig }): Promise<void>;
  close(): Promise<void>;
  write(params: { bytes: Uint8Array; source: SerialSource }): Promise<void>;
  /** 清空环形缓冲与计数，不影响串口状态。 */
  clear(): Promise<void>;
  getSnapshot(): Promise<SerialSnapshot>;
  /** 面板可见性；仅在有可见面板或等待重连时轮询串口列表。 */
  setWatching(params: { watching: boolean }): Promise<void>;
  onData: Event<SerialChunk>;
  onStatus: Event<SerialStatus>;
  onPorts: Event<SerialPortInfo[]>;
}

export const ISerialService = createServiceDescriptor<ISerialService>(ServiceChannels.Serial);
