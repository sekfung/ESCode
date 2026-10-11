import type { Event } from "@escode/rpc";
import { ServiceChannels } from "@escode/shared";
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
  /** 仅 source="agent" 时存在：发起写入的 Agent 会话。 */
  sessionId?: string;
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
  | "io"
  /** Agent 串口工具：远程 workspace、子 agent 或 Host 未提供串口服务。 */
  | "unavailable";

export type SerialState = "closed" | "opening" | "open" | "closing" | "disconnected" | "error";

/** DTR/RTS 输出状态；打开串口后初值为 DTR=1、RTS=1（与 serialport 默认一致）。 */
export interface SerialSignals {
  dtr: boolean;
  rts: boolean;
}

/** 复位脉冲预设（docs/specs/serial-port-debugger-phase3.md 第 2 节）。 */
export type SerialSignalPulse = "esp32" | "arduino";

/** 定时循环发送的进度（仅循环进行中存在）。 */
export interface SerialLoopState {
  intervalMs: number;
  /** 不存在表示无限循环。 */
  count?: number;
  sent: number;
}

/** 循环发送的最小间隔（docs/specs/serial-port-debugger-phase3.md 第 4 节）。 */
export const SERIAL_LOOP_MIN_INTERVAL_MS = 10;

export interface SerialStatus {
  state: SerialState;
  /** 循环发送进行中的进度；随状态事件推送，进度事件间隔不低于 250ms。 */
  loop?: SerialLoopState;
  /** 会话打开过之后才有；变化随状态事件广播给所有面板与 Agent。 */
  signals?: SerialSignals;
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

/** 窗口内同时活动（非 closed）的串口会话上限（docs/specs/serial-port-debugger-phase3.md）。 */
export const SERIAL_MAX_ACTIVE_SESSIONS = 4;

export interface SerialSessionSummary {
  path: string;
  status: SerialStatus;
}

/** 事件按串口路径区分：面板标签只消费自己绑定的 path。 */
export type SerialPathChunk = SerialChunk & { path: string };
export type SerialPathStatus = SerialStatus & { path: string };

export interface ISerialService {
  list(): Promise<SerialPortInfo[]>;
  /** 活动（非 closed）的串口会话。 */
  listSessions(): Promise<SerialSessionSummary[]>;
  open(params: { path: string; config: SerialConfig }): Promise<void>;
  close(params: { path: string }): Promise<void>;
  /** 返回本次写入在收发记录中的 seq（空写入返回当前 lastSeq）。 */
  write(params: {
    path: string;
    bytes: Uint8Array;
    source: SerialSource;
    sessionId?: string;
  }): Promise<{ seq: number }>;
  /** 清空该串口的环形缓冲与计数，不影响串口状态。 */
  clear(params: { path: string }): Promise<void>;
  /**
   * 设置 DTR/RTS 或执行复位脉冲（pulse 与 dtr/rts 互斥）。与打开/关闭在会话内串行：
   * 脉冲进行中的调用排到脉冲结束后执行。返回生效后的信号状态。
   */
  setSignals(params: {
    path: string;
    dtr?: boolean;
    rts?: boolean;
    pulse?: SerialSignalPulse;
  }): Promise<SerialSignals>;
  /**
   * 由 Host 定时循环发送（关闭标签不影响）。每次发送等同一次用户写入；每个会话同时最多一个任务，
   * 再次启动替换旧任务；串口关闭、断开或出错时自动停止。Agent 不能启动循环发送。
   */
  startLoop(params: {
    path: string;
    bytes: Uint8Array;
    intervalMs: number;
    count?: number;
  }): Promise<void>;
  stopLoop(params: { path: string }): Promise<void>;
  /** 未打开过或已被淘汰的路径返回 closed 空快照。 */
  getSnapshot(params: { path: string }): Promise<SerialSnapshot>;
  /** 面板可见性；仅在有可见面板或有会话等待重连时轮询串口列表。 */
  setWatching(params: { watching: boolean }): Promise<void>;
  onData: Event<SerialPathChunk>;
  onStatus: Event<SerialPathStatus>;
  onPorts: Event<SerialPortInfo[]>;
}

export const ISerialService = createServiceDescriptor<ISerialService>(ServiceChannels.Serial);
