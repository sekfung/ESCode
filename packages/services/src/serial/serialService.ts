import { Emitter } from "@zcode/rpc";
import type { BindingInterface } from "@serialport/bindings-cpp";
import type { ServiceAuthorityMode } from "@zcode/shared";
import { createServiceLogger } from "../logger/serviceLogger.js";
import {
  SERIAL_MAX_ACTIVE_SESSIONS,
  SerialError,
  type ISerialService,
  type SerialChunk,
  type SerialConfig,
  type SerialPathChunk,
  type SerialPathStatus,
  type SerialPortInfo,
  type SerialSessionSummary,
  type SerialSnapshot,
  type SerialSource,
} from "./serial.js";
import type { SerialReadResult } from "./serialChunkBuffer.js";
import { SerialSession } from "./serialSession.js";
import { waitForSerialRx, type SerialWaitResult } from "./serialWait.js";
import { loadDefaultSerialBinding } from "./serialRuntime.js";

export { loadDefaultSerialBinding } from "./serialRuntime.js";
export type { SerialWaitResult } from "./serialWait.js";

const log = createServiceLogger("serial");

const DEFAULT_POLL_INTERVAL_MS = 1500;
const DEFAULT_BUFFER_LIMIT_BYTES = 1024 * 1024;
const DEFAULT_COALESCE_WINDOW_MS = 10;
const DEFAULT_COALESCE_MAX_BYTES = 4096;
/** 会话表最多保留的会话数（含已关闭会话的历史）；超出时淘汰最早关闭的会话。 */
const MAX_RETAINED_SESSIONS = 8;

export interface CreateSerialServiceOptions {
  /** 默认延迟加载 @serialport/bindings-cpp；测试注入 mock binding。 */
  loadBinding?: () => Promise<BindingInterface>;
  pollIntervalMs?: number;
  bufferLimitBytes?: number;
  coalesceWindowMs?: number;
  coalesceMaxBytes?: number;
}

type ReadParams = {
  path: string;
  sinceSeq?: number;
  direction: SerialChunk["direction"] | "both";
  maxBytes: number;
};

/** Host 进程内接口：在 RPC 契约之外提供 Agent 串口工具使用的游标读取与等待。 */
export interface SerialService extends ISerialService {
  readSince(params: ReadParams): SerialReadResult;
  /**
   * 等待该串口 sinceSeq（缺省为调用时刻）之后的 RX 满足 test。串口不处于 open 时立即返回 disconnected；
   * 超时、取消都会释放订阅。
   */
  waitFor<T>(params: {
    path: string;
    sinceSeq?: number;
    timeoutMs: number;
    signal?: AbortSignal;
    test: (rxChunks: readonly SerialChunk[]) => T | null;
  }): Promise<SerialWaitResult<T>>;
  disposeAll(): void;
  disposeAllAndWait(): Promise<void>;
}

function portsKey(ports: SerialPortInfo[]): string {
  return ports.map((port) => port.path).join("\n");
}

const isActive = (session: SerialSession) => session.state !== "closed";

class SerialServiceImpl implements SerialService {
  private readonly dataEmitter = new Emitter<SerialPathChunk>();
  private readonly statusEmitter = new Emitter<SerialPathStatus>();
  private readonly portsEmitter = new Emitter<SerialPortInfo[]>();
  readonly onData = this.dataEmitter.event;
  readonly onStatus = this.statusEmitter.event;
  readonly onPorts = this.portsEmitter.event;

  private readonly pollIntervalMs: number;
  private readonly sessions = new Map<string, SerialSession>();
  /** 窗口内统一的 seq：跨会话、跨关闭重开都单调递增，renderer 去重永不误判。 */
  private seq = 0;
  private bindingPromise: Promise<BindingInterface> | null = null;

  private watching = false;
  private pollTimer: ReturnType<typeof setTimeout> | null = null;
  private polling = false;
  private lastPortsKey: string | null = null;
  private disposed = false;

  constructor(private readonly options: CreateSerialServiceOptions) {
    this.pollIntervalMs = options.pollIntervalMs ?? DEFAULT_POLL_INTERVAL_MS;
  }

  async list(): Promise<SerialPortInfo[]> {
    const binding = await this.getBinding();
    try {
      const ports = await binding.list();
      return ports.map((port) => ({
        path: port.path,
        manufacturer: port.manufacturer,
        serialNumber: port.serialNumber,
        vendorId: port.vendorId,
        productId: port.productId,
      }));
    } catch (error) {
      throw new SerialError("io", error instanceof Error ? error.message : String(error));
    }
  }

  async listSessions(): Promise<SerialSessionSummary[]> {
    return [...this.sessions.values()]
      .filter(isActive)
      .map((session) => ({ path: session.path, status: session.currentStatus }));
  }

  async open(params: { path: string; config: SerialConfig }): Promise<void> {
    const existing = this.sessions.get(params.path);
    const activeOthers = [...this.sessions.values()].filter(
      (session) => session.path !== params.path && isActive(session),
    ).length;
    if (activeOthers >= SERIAL_MAX_ACTIVE_SESSIONS) {
      throw new SerialError(
        "invalidInput",
        `At most ${SERIAL_MAX_ACTIVE_SESSIONS} serial ports can be open at the same time`,
      );
    }
    const session = existing ?? this.createSession(params.path);
    await session.open(params.config);
  }

  async close(params: { path: string }): Promise<void> {
    await this.sessions.get(params.path)?.close();
  }

  async write(params: {
    path: string;
    bytes: Uint8Array;
    source: SerialSource;
    sessionId?: string;
  }): Promise<{ seq: number }> {
    const session = this.sessions.get(params.path);
    if (!session) throw new SerialError("notOpen", "Serial port is not open");
    return session.write(params);
  }

  async clear(params: { path: string }): Promise<void> {
    this.sessions.get(params.path)?.clear();
  }

  async getSnapshot(params: { path: string }): Promise<SerialSnapshot> {
    const session = this.sessions.get(params.path);
    if (session) return session.snapshot();
    return {
      status: { state: "closed", path: params.path },
      chunks: [],
      seq: this.seq,
      stats: { rxBytes: 0, txBytes: 0 },
    };
  }

  readSince(params: ReadParams): SerialReadResult {
    const session = this.sessions.get(params.path);
    if (session) return session.readSince(params);
    return { chunks: [], lastSeq: params.sinceSeq ?? this.seq, truncated: false, evicted: false };
  }

  waitFor<T>(params: {
    path: string;
    sinceSeq?: number;
    timeoutMs: number;
    signal?: AbortSignal;
    test: (rxChunks: readonly SerialChunk[]) => T | null;
  }): Promise<SerialWaitResult<T>> {
    const session = this.sessions.get(params.path);
    const path = params.path;
    return waitForSerialRx(
      {
        lastSeq: () => session?.lastSeq ?? this.seq,
        rxSince: (seq) => session?.rxSince(seq) ?? [],
        isOpen: () => session?.state === "open",
        onData: (listener) => this.onData((chunk) => chunk.path === path && listener(chunk)),
        onStatus: (listener) => this.onStatus((status) => status.path === path && listener(status)),
      },
      params,
    );
  }

  async setWatching(params: { watching: boolean }): Promise<void> {
    this.watching = params.watching;
    if (this.watching) this.lastPortsKey = null;
    this.updatePolling();
  }

  disposeAll(): void {
    void this.disposeAllAndWait();
  }

  async disposeAllAndWait(): Promise<void> {
    if (this.disposed) return;
    this.disposed = true;
    this.stopPolling();
    await Promise.all(
      [...this.sessions.values()].map((session) => session.close().catch(() => {})),
    );
    this.dataEmitter.dispose();
    this.statusEmitter.dispose();
    this.portsEmitter.dispose();
  }

  // ---------------------------------------------------------------------------

  private createSession(path: string): SerialSession {
    const session = new SerialSession({
      path,
      getBinding: () => this.getBinding(),
      buffer: {
        limitBytes: this.options.bufferLimitBytes ?? DEFAULT_BUFFER_LIMIT_BYTES,
        coalesceWindowMs: this.options.coalesceWindowMs ?? DEFAULT_COALESCE_WINDOW_MS,
        coalesceMaxBytes: this.options.coalesceMaxBytes ?? DEFAULT_COALESCE_MAX_BYTES,
      },
      nextSeq: () => ++this.seq,
      onChunk: (chunk) => this.dataEmitter.fire({ ...chunk, path }),
      onStatus: (status) => {
        this.statusEmitter.fire({ ...status, path });
        this.updatePolling();
      },
      log,
    });
    this.sessions.set(path, session);
    this.evictClosedSessions();
    return session;
  }

  /** 已关闭会话保留历史；会话表超出上限时淘汰最早关闭的会话。 */
  private evictClosedSessions(): void {
    if (this.sessions.size <= MAX_RETAINED_SESSIONS) return;
    const closed = [...this.sessions.values()]
      .filter((session) => !isActive(session))
      .sort((a, b) => a.lastChangedAt - b.lastChangedAt);
    for (const session of closed) {
      if (this.sessions.size <= MAX_RETAINED_SESSIONS) return;
      this.sessions.delete(session.path);
    }
  }

  private getBinding(): Promise<BindingInterface> {
    if (!this.bindingPromise) {
      const load = this.options.loadBinding ?? loadDefaultSerialBinding;
      this.bindingPromise = load().catch((error: unknown) => {
        this.bindingPromise = null;
        const message = error instanceof Error ? error.message : String(error);
        // 原生模块缺失只影响串口功能：延迟到首次使用时报错，Host 其他服务照常注册。
        log.warn("serial native binding unavailable", { message });
        throw new SerialError("nativeUnavailable", message);
      });
    }
    return this.bindingPromise;
  }

  // --- 热插拔轮询：一次轮询服务所有会话的重连 ------------------------------------

  private hasDisconnected(): boolean {
    return [...this.sessions.values()].some((session) => session.state === "disconnected");
  }

  private shouldPoll(): boolean {
    return !this.disposed && (this.watching || this.hasDisconnected());
  }

  private updatePolling(): void {
    if (!this.shouldPoll()) {
      this.stopPolling();
      return;
    }
    if (!this.pollTimer && !this.polling) this.schedulePoll(0);
  }

  private stopPolling(): void {
    if (this.pollTimer) clearTimeout(this.pollTimer);
    this.pollTimer = null;
  }

  private schedulePoll(delayMs: number): void {
    this.pollTimer = setTimeout(() => {
      this.pollTimer = null;
      void this.pollOnce();
    }, delayMs);
  }

  private async pollOnce(): Promise<void> {
    if (!this.shouldPoll()) return;
    this.polling = true;
    try {
      const ports = await this.list();
      const key = portsKey(ports);
      if (this.watching && key !== this.lastPortsKey) {
        this.lastPortsKey = key;
        this.portsEmitter.fire(ports);
      }
      const present = new Set(ports.map((port) => port.path));
      const reconnecting = [...this.sessions.values()].filter(
        (session) => session.state === "disconnected" && present.has(session.path),
      );
      await Promise.all(reconnecting.map((session) => session.reconnect().catch(() => {})));
    } catch (error) {
      log.debug("serial port poll failed", {
        message: error instanceof Error ? error.message : String(error),
      });
    } finally {
      this.polling = false;
    }
    if (this.shouldPoll() && !this.pollTimer) this.schedulePoll(this.pollIntervalMs);
  }
}

export function createSerialService(options: CreateSerialServiceOptions = {}): SerialService {
  return new SerialServiceImpl(options);
}

/**
 * 串口设备只属于本机：仅窗口级 Desktop Local Host 拥有串口会话。
 * desktop-attached-remote / standalone-server 都不注册，Web 与远端 server 天然没有该服务。
 */
export function shouldRegisterSerialService(mode: ServiceAuthorityMode | undefined): boolean {
  return mode === "desktop-local";
}
