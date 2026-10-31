import {
  bridgeRustHostPort,
  type RustHostClientMode,
  type RustHostExit,
  type RustHostRemoteProfileRequest,
  type RustHostRemoteScope,
  cancelRustHostRemoteProfile,
  isRustHostEnabled,
  resolveRustHostRemoteProfile,
  RustHostStartError,
  startRustHost,
  type RustHostHandle,
} from "./rustHostBridge.js";
import type {
  RustHostAttachment,
  RustHostAttachmentClosedEvent,
  RustHostAttachmentLostReason,
  RustHostClock,
  RustHostManager,
  RustHostManagerOptions,
  RustHostManagerState,
  RustHostRestartPolicy,
} from "./rustHostManagerTypes.js";

export type {
  RustHostAttachment,
  RustHostAttachmentLostReason,
  RustHostClock,
  RustHostManager,
  RustHostManagerOptions,
  RustHostManagerState,
  RustHostRestartPolicy,
} from "./rustHostManagerTypes.js";

// Rust Host 生命周期（默认开启，docs/specs/rust-u2-host-skeleton.md §6；韧性见
// docs/specs/rust-u2-host-resilience.md §3/§4）：
// - Main 懒启动一个 App 级 Rust Host（每 App 一个进程，所有窗口/远程 workspace
//   经 attachment 接入同一个 Host，符合 AGENTS.md 的状态所有者约定）；显式 `ESCODE_RUST_HOST=0/false`
//   才回退 Node Host；
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
  const hostEventListeners = new Set<(event: unknown) => void>();
  const workspaceStartupListeners = new Set<
    (state: {
      startupId: string;
      windowId: string;
      workspaceKey: string;
      generation: number;
      state: unknown;
    }) => void
  >();
  const attachments = new Set<LiveAttachment>();

  function setState(next: RustHostManagerState) {
    current = next;
    for (const listener of Array.from(listeners)) listener(next);
  }

  async function launch(): Promise<RustHostHandle> {
    // Rust Host 不建库：Main 必须先按 TS 迁移路径准备好 tasks-index，再 spawn Host。
    try {
      await options.prepareStorage?.();
    } catch (error) {
      throw new RustHostStartError(
        "RUST_HOST_STORAGE_FAILED",
        `tasks-index preparation failed: ${error instanceof Error ? error.message : String(error)}`,
      );
    }
    const started = await startRustHost({
      binary: options.binary,
      ...(options.args ? { args: options.args } : {}),
      home: options.home,
      ...(options.dataBaseDir ? { dataBaseDir: options.dataBaseDir } : {}),
      ...(options.runtimeEnv ? { runtimeEnv: await options.runtimeEnv() } : {}),
      ...(options.remoteAssets ? { remoteAssets: options.remoteAssets() } : {}),
      ...(options.startTimeoutMs ? { timeoutMs: options.startTimeoutMs } : {}),
      ...(options.authorizeLocalMediaPreviewPath
        ? {
            authorizeLocalMediaPreviewPath: options.authorizeLocalMediaPreviewPath,
          }
        : {}),
      onMainRequest: options.onMainRequest,
      onHostEvent: (event) => {
        for (const listener of Array.from(hostEventListeners)) listener(event);
      },
      onWorkspaceStartupState: (state) => {
        for (const listener of Array.from(workspaceStartupListeners)) listener(state);
      },
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

  /**
   * 生产一个 attachment 端口并桥到 Rust Host（`scope` 存在时走远端 scope 的 RPC
   * WebSocket；路径由 `clientMode` 决定，见 spec §10.15）。
   *
   * 修复：旧实现把整段桥接内联在 `attachLocalPort` 里（评审后新增 `attachRemoteScopePort` 时
   * 复制一份会漂移）；这里抽成同一份，`prepare`/`scope` 只是桥接前的可选步骤。
   */
  async function attachPort(
    createChannel: () => { port1: unknown; port2: unknown },
    clientMode: RustHostClientMode,
    params: {
      prepare?: {
        startupId: string;
        windowId: string;
        workspace: {
          path: string;
          identity?: string;
          dataDir?: string;
          config?: string;
        };
        runtimeEnvPatch?: Record<string, string>;
      };
      scope?: RustHostRemoteScope;
    },
  ): Promise<RustHostAttachment> {
    const started = await ensureStarted();
    if (params.prepare) {
      started.send({
        kind: "workspacePrepare",
        startupId: params.prepare.startupId,
        windowId: params.prepare.windowId,
        workspace: params.prepare.workspace,
        ...(params.prepare.runtimeEnvPatch
          ? { runtimeEnvPatch: params.prepare.runtimeEnvPatch }
          : {}),
      });
    }
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
    const lostListeners: Array<(reason: RustHostAttachmentLostReason) => void> = [];
    let lostReason: RustHostAttachmentLostReason | undefined;
    const closedListeners: Array<(event: RustHostAttachmentClosedEvent) => void> = [];
    let closedReason: RustHostAttachmentClosedEvent | undefined;
    // 远端 scope 的 capability 申请包含 Host 建连（首次连接还会安装，spec §11.3）。
    // attachment 必须等 scope 真正就绪后再交给 renderer，否则 renderer 的注册握手会
    // 和安装耗时混在一起，被误判为 renderer 未 ready。
    let settleReady: ((error?: Error) => void) | undefined;
    const ready = params.scope
      ? new Promise<void>((resolve, reject) => {
          settleReady = (error) => {
            if (error) reject(error);
            else resolve();
          };
        })
      : undefined;
    let live!: LiveAttachment;
    const notifyLost = (reason: RustHostAttachmentLostReason) => {
      if (lostReason) return;
      lostReason = reason;
      live.teardown();
      for (const listener of lostListeners.splice(0)) listener(reason);
    };
    const notifyClosed = (event: RustHostAttachmentClosedEvent) => {
      if (closedReason) return;
      closedReason = event;
      live.teardown();
      for (const listener of closedListeners.splice(0)) listener(event);
    };
    const bridgeTeardown = bridgeRustHostPort(
      started,
      channel.port2 as never,
      clientMode,
      params.scope,
      {
        onReady: () => settleReady?.(),
        onUnexpectedClose: (event) =>
          notifyClosed({
            ...(event.code != null ? { code: event.code } : {}),
            ...(event.reason ? { reasonText: event.reason } : {}),
          }),
      },
    );
    live = {
      teardown() {
        settleReady?.(new Error("Rust Host scoped attachment closed before ready"));
        bridgeTeardown();
        channel.port1.close?.();
        channel.port2.close?.();
      },
      lost(exit) {
        notifyLost({ reason: "host-exited", exit });
      },
    };
    attachments.add(live);
    if (ready) {
      try {
        await ready;
      } catch (error) {
        attachments.delete(live);
        live.teardown();
        throw error instanceof Error ? error : new Error(String(error));
      }
    }
    return {
      port: channel.port1,
      dispose() {
        attachments.delete(live);
        live.teardown();
      },
      onLost(listener) {
        if (lostReason) listener(lostReason);
        else lostListeners.push(listener);
      },
      onClosed(listener) {
        if (closedReason) listener(closedReason);
        else closedListeners.push(listener);
      },
    };
  }

  return {
    enabled,
    ensureStarted,
    async attachLocalPort(
      createChannel: () => { port1: unknown; port2: unknown },
      clientMode: RustHostClientMode,
      prepare?: {
        startupId: string;
        windowId: string;
        workspace: {
          path: string;
          identity?: string;
          dataDir?: string;
          config?: string;
        };
        runtimeEnvPatch?: Record<string, string>;
      },
    ) {
      return attachPort(createChannel, clientMode, { prepare });
    },
    async attachRemoteScopePort(createChannel, scope, clientMode) {
      return attachPort(createChannel, clientMode, { scope });
    },
    async resolveRemoteProfile(
      request: RustHostRemoteProfileRequest,
      options?: { signal?: AbortSignal },
    ) {
      return resolveRustHostRemoteProfile(await ensureStarted(), request, options);
    },
    /**
     * 取消在途首连（spec §12.2）。不调用 `ensureStarted`：取消不应为了取消而拉起 Host。
     */
    async cancelRemoteProfile(requestId: string) {
      const current = handle;
      if (!current) return;
      await cancelRustHostRemoteProfile(current, requestId);
    },
    isRunning: () => handle !== undefined,
    sendControl(message) {
      handle?.send(message);
    },
    onHostEvent(listener) {
      hostEventListeners.add(listener);
      return () => hostEventListeners.delete(listener);
    },
    onWorkspaceStartupState(listener) {
      workspaceStartupListeners.add(listener);
      return () => workspaceStartupListeners.delete(listener);
    },
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

type HostPortLikeOn = (event: "message", listener: (event: unknown) => void) => void;

export { defaultRustHostBinary } from "./rustHostBinary.js";
