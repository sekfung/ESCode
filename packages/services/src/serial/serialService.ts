import { Emitter } from "@zcode/rpc";
import type { BindingInterface } from "@serialport/bindings-cpp";
import type { ServiceAuthorityMode } from "@zcode/shared";
import type { SerialPortStream } from "@serialport/stream";
import { createServiceLogger } from "../logger/serviceLogger.js";
import {
  SERIAL_WRITE_LIMIT_BYTES,
  SerialError,
  type ISerialService,
  type SerialChunk,
  type SerialConfig,
  type SerialPortInfo,
  type SerialSnapshot,
  type SerialSource,
  type SerialStatus,
} from "./serial.js";
import { SerialChunkBuffer } from "./serialChunkBuffer.js";
import { mapSerialOpenError } from "./serialErrors.js";

const log = createServiceLogger("serial");

const DEFAULT_POLL_INTERVAL_MS = 1500;
const DEFAULT_BUFFER_LIMIT_BYTES = 1024 * 1024;
const DEFAULT_COALESCE_WINDOW_MS = 10;
const DEFAULT_COALESCE_MAX_BYTES = 4096;

const VALID_DATA_BITS = new Set([5, 6, 7, 8]);
const VALID_PARITY = new Set(["none", "even", "odd", "mark", "space"]);
const VALID_STOP_BITS = new Set([1, 1.5, 2]);

export interface CreateSerialServiceOptions {
  /** 默认延迟加载 @serialport/bindings-cpp；测试注入 mock binding。 */
  loadBinding?: () => Promise<BindingInterface>;
  pollIntervalMs?: number;
  bufferLimitBytes?: number;
  coalesceWindowMs?: number;
  coalesceMaxBytes?: number;
}

export interface SerialService extends ISerialService {
  disposeAll(): void;
  disposeAllAndWait(): Promise<void>;
}

type SerialStreamModule = typeof import("@serialport/stream");
let streamModulePromise: Promise<SerialStreamModule> | null = null;

// @serialport/* 是 CJS 包；server 会把 services 内联进 ESM bundle，静态 import 会在启动时
// 触发 Dynamic require 崩溃。串口只在 Desktop Local Host 使用，因此全部延迟到首次打开时加载。
function loadStreamModule(): Promise<SerialStreamModule> {
  streamModulePromise ??= import("@serialport/stream").catch((error: unknown) => {
    streamModulePromise = null;
    throw new SerialError(
      "nativeUnavailable",
      error instanceof Error ? error.message : String(error),
    );
  });
  return streamModulePromise;
}

async function loadNativeBinding(): Promise<BindingInterface> {
  const module = await import("@serialport/bindings-cpp");
  return module.autoDetect();
}

/**
 * E2E 专用接缝：设置 ZCODE_SERIAL_MOCK_PORTS（逗号分隔）时改用 binding-mock 的回环虚拟串口，
 * 不触碰真实硬件。binding-mock 只是开发依赖，安装包里不存在，生产环境即使误设也只会报 nativeUnavailable。
 */
async function loadMockBinding(paths: string[]): Promise<BindingInterface> {
  // binding-mock 的 exports 未声明 types，且已在打包配置中外置；用变量模块名按运行时依赖加载。
  const specifier = "@serialport/binding-mock";
  const { MockBinding } = (await import(specifier)) as {
    MockBinding: BindingInterface & {
      createPort(path: string, options?: { echo?: boolean; manufacturer?: string }): void;
    };
  };
  const existing = new Set((await MockBinding.list()).map((port) => port.path));
  for (const path of paths) {
    if (!existing.has(path)) MockBinding.createPort(path, { echo: true, manufacturer: "Mock" });
  }
  return {
    list: async () => (await MockBinding.list()).filter((port) => paths.includes(port.path)),
    open: (options) => MockBinding.open(options),
  };
}

export function loadDefaultSerialBinding(): Promise<BindingInterface> {
  const mockPorts = process.env.ZCODE_SERIAL_MOCK_PORTS?.split(",")
    .map((path) => path.trim())
    .filter(Boolean);
  return mockPorts?.length ? loadMockBinding(mockPorts) : loadNativeBinding();
}

function validateConfig(path: string, config: SerialConfig): void {
  const problems: string[] = [];
  if (typeof path !== "string" || path.trim() === "") problems.push("path");
  if (!Number.isInteger(config.baudRate) || config.baudRate <= 0) problems.push("baudRate");
  if (!VALID_DATA_BITS.has(config.dataBits)) problems.push("dataBits");
  if (!VALID_PARITY.has(config.parity)) problems.push("parity");
  if (!VALID_STOP_BITS.has(config.stopBits)) problems.push("stopBits");
  if (problems.length > 0) {
    throw new SerialError("invalidConfig", `Invalid serial config: ${problems.join(", ")}`);
  }
}

function portsKey(ports: SerialPortInfo[]): string {
  return ports.map((port) => port.path).join("\n");
}

class SerialServiceImpl implements SerialService {
  private readonly dataEmitter = new Emitter<SerialChunk>();
  private readonly statusEmitter = new Emitter<SerialStatus>();
  private readonly portsEmitter = new Emitter<SerialPortInfo[]>();
  readonly onData = this.dataEmitter.event;
  readonly onStatus = this.statusEmitter.event;
  readonly onPorts = this.portsEmitter.event;

  private readonly pollIntervalMs: number;
  private readonly buffer: SerialChunkBuffer;

  private bindingPromise: Promise<BindingInterface> | null = null;
  private status: SerialStatus = { state: "closed" };
  private port: SerialPortStream | null = null;

  /** open/close/重连串行执行；状态转换只在队列里发生。 */
  private queue: Promise<void> = Promise.resolve();
  private inFlightOpen: { key: string; promise: Promise<void> } | null = null;

  private watching = false;
  private pollTimer: ReturnType<typeof setTimeout> | null = null;
  private polling = false;
  private lastPortsKey: string | null = null;
  private disposed = false;

  constructor(private readonly options: CreateSerialServiceOptions) {
    this.pollIntervalMs = options.pollIntervalMs ?? DEFAULT_POLL_INTERVAL_MS;
    this.buffer = new SerialChunkBuffer({
      limitBytes: options.bufferLimitBytes ?? DEFAULT_BUFFER_LIMIT_BYTES,
      coalesceWindowMs: options.coalesceWindowMs ?? DEFAULT_COALESCE_WINDOW_MS,
      coalesceMaxBytes: options.coalesceMaxBytes ?? DEFAULT_COALESCE_MAX_BYTES,
      onChunk: (chunk) => this.dataEmitter.fire(chunk),
    });
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

  open(params: { path: string; config: SerialConfig }): Promise<void> {
    const key = JSON.stringify([params.path, params.config]);
    if (this.inFlightOpen?.key === key) return this.inFlightOpen.promise;
    const promise = this.enqueue(() => this.openNow(params.path, params.config));
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

  async write(params: { bytes: Uint8Array; source: SerialSource }): Promise<void> {
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
    if (params.bytes.byteLength === 0) return;
    const bytes = Buffer.from(params.bytes);
    this.buffer.pushTx(params.source, bytes);
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
  }

  async clear(): Promise<void> {
    this.buffer.clear();
  }

  async getSnapshot(): Promise<SerialSnapshot> {
    return { status: { ...this.status }, ...this.buffer.snapshot() };
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
    await this.enqueue(() => this.closeNow()).catch(() => {});
    this.dataEmitter.dispose();
    this.statusEmitter.dispose();
    this.portsEmitter.dispose();
  }

  // ---------------------------------------------------------------------------

  private enqueue(operation: () => Promise<void>): Promise<void> {
    const run = this.queue.then(operation);
    this.queue = run.catch(() => {});
    return run;
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

  private setStatus(status: SerialStatus): void {
    this.status = status;
    this.statusEmitter.fire({ ...status });
    this.updatePolling();
  }

  private async openNow(path: string, config: SerialConfig): Promise<void> {
    validateConfig(path, config);
    const binding = await this.getBinding();
    if (this.status.state === "open" && this.status.path === path) {
      if (JSON.stringify(this.status.config) === JSON.stringify(config)) return;
    }
    if (this.port || this.status.state === "disconnected") await this.closeNow();
    await this.openPort(binding, path, config);
  }

  private async openPort(binding: BindingInterface, path: string, config: SerialConfig) {
    this.setStatus({ state: "opening", path, config });
    let SerialPortStreamClass: SerialStreamModule["SerialPortStream"];
    try {
      SerialPortStreamClass = (await loadStreamModule()).SerialPortStream;
    } catch (error) {
      const mapped = mapSerialOpenError(error);
      this.setStatus({
        state: "error",
        path,
        config,
        error: { code: mapped.code, message: mapped.message },
      });
      throw mapped;
    }
    const port = new SerialPortStreamClass({
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
      const mapped = mapSerialOpenError(error);
      log.warn("serial open failed", {
        path,
        code: mapped.code,
        message: mapped.message,
      });
      this.setStatus({
        state: "error",
        path,
        config,
        error: { code: mapped.code, message: mapped.message },
      });
      throw mapped;
    }
    this.port = port;
    port.on("data", (data: Buffer) => {
      if (this.port === port) this.buffer.receive(data);
    });
    port.on("error", (error: Error) => {
      log.warn("serial port error", { path, message: error.message });
    });
    port.on("close", (error?: Error & { disconnected?: boolean }) => {
      if (this.port !== port || !error?.disconnected) return;
      this.handleDisconnect(path, config);
    });
    log.info("serial port opened", { path, baudRate: config.baudRate });
    this.setStatus({ state: "open", path, config });
  }

  private async closeNow(): Promise<void> {
    const port = this.port;
    const { path, config } = this.status;
    if (port) {
      this.setStatus({ state: "closing", path, config });
      this.port = null;
      this.buffer.flushRx();
      if (port.isOpen) {
        await new Promise<void>((resolve) => {
          port.close((error) => {
            if (error) log.warn("serial close failed", { path, message: error.message });
            resolve();
          });
        });
      }
      port.removeAllListeners();
      log.info("serial port closed", { path });
      this.setStatus({ state: "closed", path, config });
      return;
    }
    if (this.status.state !== "closed") {
      this.setStatus({ state: "closed", path, config });
    }
  }

  private handleDisconnect(path: string, config: SerialConfig): void {
    const port = this.port;
    this.port = null;
    port?.removeAllListeners();
    this.buffer.flushRx();
    log.info("serial device disconnected", {
      path,
      autoReconnect: config.autoReconnect,
    });
    this.setStatus({
      state: config.autoReconnect ? "disconnected" : "closed",
      path,
      config,
    });
  }

  private reconnect(path: string): Promise<void> {
    return this.enqueue(async () => {
      // 入队后可能已被用户关闭或改开其他串口；只有仍在等待同一设备时才重连。
      const { state, config } = this.status;
      if (state !== "disconnected" || this.status.path !== path || !config) return;
      const binding = await this.getBinding();
      await this.openPort(binding, path, config);
      log.info("serial device reconnected", { path });
    });
  }

  // --- 热插拔轮询 ---------------------------------------------------------------

  private shouldPoll(): boolean {
    return !this.disposed && (this.watching || this.status.state === "disconnected");
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
      const { state, path } = this.status;
      if (state === "disconnected" && path && ports.some((port) => port.path === path)) {
        await this.reconnect(path).catch(() => {});
      }
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
