import { join } from "node:path";
import {
  bridgeRustHostPort,
  type RustHostClientMode,
  type RustHostExit,
  isRustHostEnabled,
  RustHostStartError,
  startRustHost,
  type RustHostHandle,
} from "./rustHostBridge.js";

// Rust Host 生命周期（dev 开关，docs/specs/rust-u2-host-skeleton.md §6；韧性见
// docs/specs/rust-u2-host-resilience.md §3/§4）：
// - `ESCODE_RUST_HOST=1` 时 Main 懒启动一个 App 级 Rust Host（每 App 一个进程，所有窗口/远程 workspace
//   经 attachment 接入同一个 Host，符合 AGENTS.md 的状态所有者约定）；
// - `attachLocalPort`：给一个 attachment 生产 renderer/relay 侧的 MessagePort——Main 把另一端桥到
//   Rust Host 的 RPC WebSocket，因此**消费方协议不变**（MessagePort 上的 `@escode/rpc`）；
// - Host 意外退出（Main 没有调用 `stop()`）时：已桥接的 attachment 全部 `onLost`（消费方按
//   web-remote-replayable 恢复重建），并按“60 s 内最多 3 次、退避 500 ms / 2 s / 5 s”自动重启；
//   超过上限进入 `failed`，`ensureStarted()` 以 `RUST_HOST_CRASH_LOOP` 显式拒绝，不回退 Node Host。
//
// 未接线处（下一步，见 u2 子 spec）：桌面窗口自身 host 的进程无关化——
// `desktopHostProcess.ts:762` 的 `MessageChannelMain` 已把 `port1` 交给 renderer、`port2` 交给
// Electron utility Host；切到 Rust Host 需要 `windowHostProcessMap`/broadcast/taskRealtime 等
// 子系统接受"非 utility process 的 window host"，属独立改造。

export interface RustHostManagerOptions {
  /** Host 可执行文件路径。 */
  binary: string;
  /** Host 可执行文件参数（测试用；缺省无参数）。 */
  args?: string[];
  /** 设置域根目录（Main 决定）。 */
  home: string;
  /** 应用数据根（TS `getDataBaseDir()`，Main 决定；供会话工作区等路径派生）。 */
  dataBaseDir?: string;
  /** 嵌入运行时的环境（Main 合成，见 `buildRustHostRuntimeEnv`）；懒启动时才求值，避免启动期读取。 */
  runtimeEnv?: () => Record<string, string> | Promise<Record<string, string>>;
  /** 本地媒体路径授权（Main 的注册表）；提供后 Host 的 media-preview 回 `local-url`（见 rustHostBridge）。 */
  authorizeLocalMediaPreviewPath?: (path: string) => Promise<string>;
  /** 显式覆盖开关（默认读 `ESCODE_RUST_HOST`）。 */
  enabled?: boolean;
  /** 启动握手截止时间（默认 10s，spec §4）。 */
  startTimeoutMs?: number;
  /** 受限重启策略（默认 60 s 内 3 次，退避 500 ms / 2 s / 5 s；spec §3.2）。 */
  restartPolicy?: Partial<RustHostRestartPolicy>;
  /** 时钟与定时器（测试注入；默认 `Date.now` / `setTimeout`）。 */
  clock?: RustHostClock;
  /** 日志钩子。`error` 用于启动失败、意外退出与放弃重启（缺省退回 `warn`）。 */
  log?: {
    info: (...args: unknown[]) => void;
    warn: (...args: unknown[]) => void;
    error?: (...args: unknown[]) => void;
  };
}

export interface RustHostRestartPolicy {
  /** 窗口内允许的自动重启次数。 */
  maxRestarts: number;
  /** 计数窗口（毫秒）。 */
  windowMs: number;
  /** 第 n 次重启之前的等待（毫秒，下标 n-1；超出取最后一个）。 */
  backoffMs: number[];
}

export interface RustHostClock {
  now(): number;
  setTimeout(callback: () => void, ms: number): unknown;
  clearTimeout(timer: unknown): void;
}

/** 管理器状态（spec §3.2 状态机）。 */
export type RustHostManagerState =
  | { state: "idle" }
  | { state: "starting" }
  | { state: "running"; pid: number | undefined }
  | { state: "restarting"; attempt: number; delayMs: number; exit: RustHostExit }
  | { state: "failed"; error: RustHostStartError };

export interface RustHostAttachment {
  /** 交给 renderer/relay 的端口（另一端已在 Main 桥到 Rust Host）。 */
  port: unknown;
  /** 解绑：关闭桥与端口。 */
  dispose(): void;
  /**
   * Host 意外退出时回调一次（端口已关闭、桥已拆）。消费方据此释放并按自己的恢复语义重建
   * （spec §3.4：不在原端口上原地重连）。
   */
  onLost(listener: (reason: { reason: "host-exited"; exit: RustHostExit }) => void): void;
}

export interface RustHostManager {
  /** 开关是否打开（关闭时调用方走 Node Host；M5 删除）。 */
  readonly enabled: boolean;
  /** 懒启动（single-flight）；失败抛 `RustHostStartError`。重启退避期间等待同一次重启。 */
  ensureStarted(): Promise<RustHostHandle>;
  /**
   * 生产一个 attachment 端口（内部 `ensureStarted` + 桥接）。`clientMode` 与 Node 路径同一
   * attachment 的交付模式一致（远程 workspace共享 Host = `web-remote-replayable`）。
   */
  attachLocalPort(
    createChannel: () => { port1: unknown; port2: unknown },
    clientMode: RustHostClientMode,
  ): Promise<RustHostAttachment>;
  /** 当前是否已有 Host 进程在跑（诊断/测试）。 */
  isRunning(): boolean;
  /** 当前状态（诊断/测试）。 */
  state(): RustHostManagerState;
  /** 监听状态变化（返回取消函数）。 */
  onStateChange(listener: (state: RustHostManagerState) => void): () => void;
  /** 放弃（`failed`）后的显式恢复入口：清空重启计数并回到 `idle`。 */
  resetRestartBudget(): void;
  /** 收口（应用退出）。 */
  stop(): Promise<void>;
}

const DEFAULT_RESTART_POLICY: RustHostRestartPolicy = {
  maxRestarts: 3,
  windowMs: 60_000,
  backoffMs: [500, 2_000, 5_000],
};

const SYSTEM_CLOCK: RustHostClock = {
  now: () => Date.now(),
  setTimeout: (callback, ms) => setTimeout(callback, ms),
  clearTimeout: (timer) => clearTimeout(timer as ReturnType<typeof setTimeout>),
};

interface LiveAttachment {
  teardown(): void;
  lost(exit: RustHostExit): void;
}

export function createRustHostManager(options: RustHostManagerOptions): RustHostManager {
  const enabled = options.enabled ?? isRustHostEnabled();
  const policy = { ...DEFAULT_RESTART_POLICY, ...options.restartPolicy };
  const clock = options.clock ?? SYSTEM_CLOCK;
  const logError = (text: string) => (options.log?.error ?? options.log?.warn)?.(text);
  let handle: RustHostHandle | undefined;
  let starting: Promise<RustHostHandle> | undefined;
  let current: RustHostManagerState = { state: "idle" };
  /** 窗口内已发生的自动重启时刻。 */
  let restarts: number[] = [];
  let restartTimer: unknown;
  /** 唤醒退避中的重启（`stop()` 用它让等待方立即收到“正在收口”而不是挂起）。 */
  let wakeRestart: (() => void) | undefined;
  /** Main 主动收口：此后的退出不算意外（spec §3.1 唯一判据）。 */
  let stopping = false;
  const listeners = new Set<(state: RustHostManagerState) => void>();
  const attachments = new Set<LiveAttachment>();

  function setState(next: RustHostManagerState) {
    current = next;
    for (const listener of Array.from(listeners)) listener(next);
  }

  async function launch(): Promise<RustHostHandle> {
    const started = await startRustHost({
      binary: options.binary,
      ...(options.args ? { args: options.args } : {}),
      home: options.home,
      ...(options.dataBaseDir ? { dataBaseDir: options.dataBaseDir } : {}),
      ...(options.runtimeEnv ? { runtimeEnv: await options.runtimeEnv() } : {}),
      ...(options.startTimeoutMs ? { timeoutMs: options.startTimeoutMs } : {}),
      ...(options.authorizeLocalMediaPreviewPath
        ? { authorizeLocalMediaPreviewPath: options.authorizeLocalMediaPreviewPath }
        : {}),
      // Host 的结构化日志（`logBatch` 控制帧）进同一条 Main 日志出口；scope 作为来源标签。
      onLogBatch: (batch) => {
        for (const line of batch.lines) {
          const text = `[rust-host] (${batch.scope}) ${line}`;
          if (batch.level === "error") logError(text);
          else if (batch.level === "warn") options.log?.warn(text);
          else if (batch.level === "info") options.log?.info(text);
          // `debug`（协议原始数据/逐条更新类高频诊断）生产不落盘，Host 侧目前也不产生，不转发。
        }
      },
    });
    handle = started;
    starting = undefined;
    setState({ state: "running", pid: started.pid });
    options.log?.info(`[rust-host] started at ${started.addr} (pid ${String(started.pid)})`);
    started.onExit((exit) => {
      if (stopping || handle !== started) return;
      onUnexpectedExit(exit);
    });
    return started;
  }

  function startFailed(error: unknown): never {
    const classified =
      error instanceof RustHostStartError
        ? error
        : new RustHostStartError("RUST_HOST_EXITED_EARLY", String(error));
    logError(`[rust-host] start failed (${classified.code}): ${classified.message}`);
    throw classified;
  }

  function crashLoop(exit: RustHostExit): RustHostStartError {
    return new RustHostStartError(
      "RUST_HOST_CRASH_LOOP",
      `Rust Host exited unexpectedly ${policy.maxRestarts + 1} times within ${policy.windowMs}ms; automatic restart stopped`,
      { exitCode: exit.code, signal: exit.signal },
    );
  }

  /** Host 意外退出：attachment 全部丢失，按策略退避重启或放弃（spec §3.2/§3.4）。 */
  function onUnexpectedExit(exit: RustHostExit) {
    handle = undefined;
    for (const attachment of Array.from(attachments)) attachment.lost(exit);
    attachments.clear();
    const now = clock.now();
    restarts = restarts.filter((at) => now - at < policy.windowMs);
    if (restarts.length >= policy.maxRestarts) {
      const error = crashLoop(exit);
      starting = undefined;
      logError(
        `[rust-host] ${error.message} (last exit code ${String(exit.code)}, signal ${String(exit.signal)})`,
      );
      setState({ state: "failed", error });
      return;
    }
    restarts.push(now);
    const attempt = restarts.length;
    const delayMs = policy.backoffMs[Math.min(attempt, policy.backoffMs.length) - 1] ?? 0;
    logError(
      `[rust-host] exited unexpectedly (code ${String(exit.code)}, signal ${String(exit.signal)}); restart ${attempt}/${policy.maxRestarts} in ${delayMs}ms`,
    );
    setState({ state: "restarting", attempt, delayMs, exit });
    // 退避期间的 ensureStarted 等这同一次重启（single-flight）；重启拉起失败也计一次。
    starting = new Promise<void>((resolve) => {
      wakeRestart = resolve;
      restartTimer = clock.setTimeout(() => {
        restartTimer = undefined;
        wakeRestart = undefined;
        resolve();
      }, delayMs);
    }).then(() => {
      if (stopping) throw new RustHostStartError("RUST_HOST_EXITED_EARLY", "Rust Host is stopping");
      return launch().catch((error: unknown) => {
        starting = undefined;
        const classified =
          error instanceof RustHostStartError
            ? error
            : new RustHostStartError("RUST_HOST_EXITED_EARLY", String(error));
        logError(
          `[rust-host] restart ${attempt} failed (${classified.code}): ${classified.message}`,
        );
        onUnexpectedExit({
          code: classified.details.exitCode ?? null,
          signal: classified.details.signal ?? null,
        });
        return ensureStarted();
      });
    });
    // 无人等待时不让拒绝变成未处理（等待方自己会收到同一个结果）。
    starting.catch(() => undefined);
  }

  async function ensureStarted(): Promise<RustHostHandle> {
    if (handle) return handle;
    if (current.state === "failed") throw current.error;
    if (starting) return starting;
    stopping = false;
    setState({ state: "starting" });
    starting = launch().catch((error: unknown) => {
      starting = undefined;
      setState({ state: "idle" });
      return startFailed(error);
    });
    return starting;
  }

  return {
    enabled,
    ensureStarted,
    async attachLocalPort(createChannel, clientMode) {
      const started = await ensureStarted();
      const channel = createChannel() as {
        port1: {
          on?: HostPortLikeOn;
          postMessage(value: unknown): void;
          start?(): void;
          close?(): void;
        };
        port2: {
          on?: HostPortLikeOn;
          postMessage(value: unknown): void;
          start?(): void;
          close?(): void;
        };
      };
      const bridgeTeardown = bridgeRustHostPort(started, channel.port2 as never, clientMode);
      const lostListeners: Array<(reason: { reason: "host-exited"; exit: RustHostExit }) => void> =
        [];
      let lostWith: RustHostExit | undefined;
      const live: LiveAttachment = {
        teardown() {
          bridgeTeardown();
          channel.port1.close?.();
          channel.port2.close?.();
        },
        lost(exit) {
          live.teardown();
          lostWith = exit;
          for (const listener of lostListeners.splice(0)) {
            listener({ reason: "host-exited", exit });
          }
        },
      };
      attachments.add(live);
      return {
        port: channel.port1,
        dispose() {
          attachments.delete(live);
          live.teardown();
        },
        onLost(listener) {
          if (lostWith) listener({ reason: "host-exited", exit: lostWith });
          else lostListeners.push(listener);
        },
      };
    },
    isRunning: () => handle !== undefined,
    state: () => current,
    onStateChange(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    resetRestartBudget() {
      restarts = [];
      if (current.state === "failed") setState({ state: "idle" });
    },
    async stop() {
      stopping = true;
      if (restartTimer !== undefined) clock.clearTimeout(restartTimer);
      restartTimer = undefined;
      wakeRestart?.();
      wakeRestart = undefined;
      const started = handle;
      handle = undefined;
      starting = undefined;
      for (const attachment of Array.from(attachments)) attachment.teardown();
      attachments.clear();
      await started?.stop();
      setState({ state: "idle" });
    },
  };
}

/** 默认的 Host 二进制路径：开发态取仓库的 target/debug，安装包内与主进程同级。 */
export function defaultRustHostBinary(appPath: string, packaged: boolean): string {
  const name = process.platform === "win32" ? "escode-host.exe" : "escode-host";
  if (packaged) return join(appPath, name);
  return join(appPath, "..", "..", "apps", "escode-host", "target", "debug", name);
}

type HostPortLikeOn = (event: "message", listener: (event: unknown) => void) => void;
