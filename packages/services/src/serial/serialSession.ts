import type { BindingInterface } from "@serialport/bindings-cpp";
import type { SerialPortStream } from "@serialport/stream";
import type { ServiceLogger } from "../logger/serviceLogger.js";
import {
  SERIAL_LOOP_MIN_INTERVAL_MS,
  SERIAL_WRITE_LIMIT_BYTES,
  SerialError,
  type SerialChunk,
  type SerialConfig,
  type SerialSignalPulse,
  type SerialSignals,
  type SerialSnapshot,
  type SerialSource,
  type SerialStatus,
} from "./serial.js";
import {
  DEFAULT_SERIAL_SIGNALS,
  isDefaultSerialSignals,
  serialPulseSteps,
} from "./serialSignals.js";
import { SerialLoopRunner } from "./serialLoop.js";
import { SerialChunkBuffer, type SerialReadResult } from "./serialChunkBuffer.js";
import { mapSerialOpenError } from "./serialErrors.js";
import { loadStreamModule, validateConfig, type SerialStreamModule } from "./serialRuntime.js";

export interface SerialSessionOptions {
  path: string;
  getBinding: () => Promise<BindingInterface>;
  buffer: { limitBytes: number; coalesceWindowMs: number; coalesceMaxBytes: number };
  nextSeq: () => number;
  onChunk: (chunk: SerialChunk) => void;
  onStatus: (status: SerialStatus) => void;
  log: ServiceLogger;
}

/**
 * 单个串口路径的会话（docs/specs/serial-port-debugger-phase3.md）：端口句柄、状态机、
 * 收发缓冲与自动重连都只在这里。open/close/重连在会话内串行执行，不同会话互不阻塞。
 */
export class SerialSession {
  readonly path: string;
  private readonly buffer: SerialChunkBuffer;
  private status: SerialStatus;
  private port: SerialPortStream | null = null;
  /** DTR/RTS 当前输出；重连后按断开前的状态恢复。 */
  private signals: SerialSignals = { ...DEFAULT_SERIAL_SIGNALS };
  private loop: SerialLoopRunner | null = null;
  private queue: Promise<void> = Promise.resolve();
  private inFlightOpen: { key: string; promise: Promise<void> } | null = null;
  /** 最近一次状态变化的时间，用于淘汰最早关闭的会话。 */
  lastChangedAt = Date.now();

  constructor(private readonly options: SerialSessionOptions) {
    this.path = options.path;
    this.status = { state: "closed", path: options.path };
    this.buffer = new SerialChunkBuffer({
      ...options.buffer,
      nextSeq: options.nextSeq,
      onChunk: options.onChunk,
    });
  }

  get state(): SerialStatus["state"] {
    return this.status.state;
  }

  get currentStatus(): SerialStatus {
    return { ...this.status };
  }

  get lastSeq(): number {
    return this.buffer.lastSeq;
  }

  open(config: SerialConfig): Promise<void> {
    const key = JSON.stringify(config);
    if (this.inFlightOpen?.key === key) return this.inFlightOpen.promise;
    const promise = this.enqueue(() => this.openNow(config));
    const entry = { key, promise };
    this.inFlightOpen = entry;
    void promise
      .catch(() => {})
      .finally(() => {
        if (this.inFlightOpen === entry) this.inFlightOpen = null;
      });
    return promise;
  }

  close(): Promise<void> {
    return this.enqueue(() => this.closeNow());
  }

  /** 设备重新出现时由 Service 的轮询调用；入队后仍处于 disconnected 才重连。 */
  reconnect(): Promise<void> {
    return this.enqueue(async () => {
      const { state, config } = this.status;
      if (state !== "disconnected" || !config) return;
      await this.openPort(await this.options.getBinding(), config);
      this.options.log.info("serial device reconnected", { path: this.path });
    });
  }

  async write(params: {
    bytes: Uint8Array;
    source: SerialSource;
    sessionId?: string;
  }): Promise<{ seq: number }> {
    const port = this.port;
    if (this.status.state !== "open" || !port) {
      throw new SerialError("notOpen", "Serial port is not open");
    }
    if (params.bytes.byteLength > SERIAL_WRITE_LIMIT_BYTES) {
      throw new SerialError(
        "invalidInput",
        `Write exceeds ${SERIAL_WRITE_LIMIT_BYTES} bytes: ${params.bytes.byteLength}`,
      );
    }
    if (params.bytes.byteLength === 0) return { seq: this.buffer.lastSeq };
    const bytes = Buffer.from(params.bytes);
    this.buffer.pushTx(
      params.source,
      bytes,
      params.source === "agent" ? params.sessionId : undefined,
    );
    const seq = this.buffer.lastSeq;
    await new Promise<void>((resolve, reject) => {
      port.write(bytes, (error) => {
        if (error) {
          reject(new SerialError("io", error.message));
          return;
        }
        port.drain((drainError) => {
          if (drainError) reject(new SerialError("io", drainError.message));
          else resolve();
        });
      });
    });
    return { seq };
  }

  setSignals(params: {
    dtr?: boolean;
    rts?: boolean;
    pulse?: SerialSignalPulse;
  }): Promise<SerialSignals> {
    let applied: SerialSignals = this.signals;
    return this.enqueue(async () => {
      if (this.status.state !== "open" || !this.port) {
        throw new SerialError("notOpen", "Serial port is not open");
      }
      if (params.rts !== undefined && this.status.config?.rtscts) {
        throw new SerialError("invalidInput", "RTS is controlled by RTS/CTS flow control");
      }
      const previous = { ...this.signals };
      if (params.pulse) {
        for (const step of serialPulseSteps(params.pulse, previous)) {
          await this.applySignals(step.signals);
          if (step.holdMs > 0) await new Promise((resolve) => setTimeout(resolve, step.holdMs));
        }
        // 脉冲结束后恢复脉冲前的开关状态。
        await this.applySignals(previous);
      } else {
        await this.applySignals({
          dtr: params.dtr ?? previous.dtr,
          rts: params.rts ?? previous.rts,
        });
      }
      applied = { ...this.signals };
      this.setStatus({ ...this.status });
    }).then(() => applied);
  }

  startLoop(params: { bytes: Uint8Array; intervalMs: number; count?: number }): void {
    if (this.status.state !== "open" || !this.port) {
      throw new SerialError("notOpen", "Serial port is not open");
    }
    const invalid =
      !Number.isInteger(params.intervalMs) ||
      params.intervalMs < SERIAL_LOOP_MIN_INTERVAL_MS ||
      params.bytes.byteLength === 0 ||
      params.bytes.byteLength > SERIAL_WRITE_LIMIT_BYTES ||
      (params.count !== undefined && (!Number.isInteger(params.count) || params.count < 1));
    if (invalid) throw new SerialError("invalidInput", "Invalid loop parameters");
    this.loop?.stop();
    const runner = new SerialLoopRunner({
      bytes: params.bytes,
      intervalMs: params.intervalMs,
      ...(params.count !== undefined ? { count: params.count } : {}),
      write: (bytes) => this.write({ bytes, source: "user" }),
      onProgress: () => {
        if (this.loop === runner) this.setStatus({ ...this.status });
      },
      onFinished: () => {
        if (this.loop !== runner) return;
        this.loop = null;
        this.setStatus({ ...this.status });
      },
    });
    this.loop = runner;
    runner.start();
  }

  stopLoop(): void {
    if (!this.loop) return;
    this.loop.stop();
    this.loop = null;
    this.setStatus({ ...this.status });
  }

  clear(): void {
    this.buffer.clear();
  }

  snapshot(): SerialSnapshot {
    return { status: this.currentStatus, ...this.buffer.snapshot() };
  }

  readSince(params: {
    sinceSeq?: number;
    direction: SerialChunk["direction"] | "both";
    maxBytes: number;
  }): SerialReadResult {
    return this.buffer.readSince(params);
  }

  rxSince(sinceSeq: number): SerialChunk[] {
    return this.buffer.rxSince(sinceSeq);
  }

  // ---------------------------------------------------------------------------

  private enqueue(operation: () => Promise<void>): Promise<void> {
    const run = this.queue.then(operation);
    this.queue = run.catch(() => {});
    return run;
  }

  private async applySignals(signals: SerialSignals): Promise<void> {
    const port = this.port;
    if (!port) throw new SerialError("notOpen", "Serial port is not open");
    await new Promise<void>((resolve, reject) => {
      port.set(signals, (error) =>
        error ? reject(new SerialError("io", error.message)) : resolve(),
      );
    });
    this.signals = { ...signals };
  }

  private setStatus(status: Omit<SerialStatus, "path">): void {
    // 离开 open（关闭、断开、出错）时循环发送随之停止。
    if (status.state !== "open" && this.loop) {
      this.loop.stop();
      this.loop = null;
    }
    const { signals: _signals, loop: _loop, ...rest } = status;
    this.status = {
      ...rest,
      path: this.path,
      signals: { ...this.signals },
      ...(this.loop ? { loop: this.loop.state } : {}),
    };
    this.lastChangedAt = Date.now();
    this.options.onStatus({ ...this.status });
  }

  private async openNow(config: SerialConfig): Promise<void> {
    validateConfig(this.path, config);
    const binding = await this.options.getBinding();
    if (
      this.status.state === "open" &&
      JSON.stringify(this.status.config) === JSON.stringify(config)
    ) {
      return;
    }
    if (this.port || this.status.state === "disconnected") await this.closeNow();
    // 用户主动打开时信号回到默认值；自动重连（reconnect）则保留断开前的状态。
    this.signals = { ...DEFAULT_SERIAL_SIGNALS };
    await this.openPort(binding, config);
  }

  private async openPort(binding: BindingInterface, config: SerialConfig): Promise<void> {
    const { path, log } = { path: this.path, log: this.options.log };
    this.setStatus({ state: "opening", config });
    const fail = (error: unknown): never => {
      const mapped = mapSerialOpenError(error);
      log.warn("serial open failed", { path, code: mapped.code, message: mapped.message });
      this.setStatus({
        state: "error",
        config,
        error: { code: mapped.code, message: mapped.message },
      });
      throw mapped;
    };
    let SerialPortStreamClass: SerialStreamModule["SerialPortStream"] | undefined;
    try {
      SerialPortStreamClass = (await loadStreamModule()).SerialPortStream;
    } catch (error) {
      fail(error);
    }
    const port = new SerialPortStreamClass!({
      binding,
      path,
      baudRate: config.baudRate,
      dataBits: config.dataBits,
      parity: config.parity,
      stopBits: config.stopBits,
      rtscts: config.rtscts,
      autoOpen: false,
    });
    try {
      await new Promise<void>((resolve, reject) => {
        port.open((error) => (error ? reject(error) : resolve()));
      });
    } catch (error) {
      fail(error);
    }
    this.port = port;
    if (!isDefaultSerialSignals(this.signals)) {
      try {
        await this.applySignals(this.signals);
      } catch (error) {
        log.warn("serial signals restore failed", {
          path,
          message: error instanceof Error ? error.message : String(error),
        });
      }
    }
    port.on("data", (data: Buffer) => {
      if (this.port === port) this.buffer.receive(data);
    });
    port.on("error", (error: Error) => {
      log.warn("serial port error", { path, message: error.message });
    });
    port.on("close", (error?: Error & { disconnected?: boolean }) => {
      if (this.port !== port || !error?.disconnected) return;
      this.handleDisconnect(config);
    });
    log.info("serial port opened", { path, baudRate: config.baudRate });
    this.setStatus({ state: "open", config });
  }

  private async closeNow(): Promise<void> {
    const port = this.port;
    const { config } = this.status;
    if (port) {
      this.setStatus({ state: "closing", config });
      this.port = null;
      this.buffer.flushRx();
      if (port.isOpen) {
        await new Promise<void>((resolve) => {
          port.close((error) => {
            if (error) {
              this.options.log.warn("serial close failed", {
                path: this.path,
                message: error.message,
              });
            }
            resolve();
          });
        });
      }
      port.removeAllListeners();
      // 修复：关闭时被取消的在途写入会在之后补发 error 事件；移除全部监听后无人接收会成为
      // 未捕获异常并打崩 Host 进程（循环发送时稳定复现）。保留一个空监听吞掉迟到的 error。
      port.on("error", () => {});
      this.options.log.info("serial port closed", { path: this.path });
      this.setStatus({ state: "closed", config });
      return;
    }
    if (this.status.state !== "closed") this.setStatus({ state: "closed", config });
  }

  private handleDisconnect(config: SerialConfig): void {
    const port = this.port;
    this.port = null;
    port?.removeAllListeners();
    // 同上：断开后迟到的 error 事件不能成为未捕获异常。
    port?.on("error", () => {});
    this.buffer.flushRx();
    this.options.log.info("serial device disconnected", {
      path: this.path,
      autoReconnect: config.autoReconnect,
    });
    this.setStatus({ state: config.autoReconnect ? "disconnected" : "closed", config });
  }
}
