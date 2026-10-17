import { join } from "node:path";
import {
  bridgeRustHostPort,
  type RustHostClientMode,
  isRustHostEnabled,
  RustHostStartError,
  startRustHost,
  type RustHostHandle,
} from "./rustHostBridge.js";

// Rust Host 生命周期（dev 开关，docs/specs/rust-u2-host-skeleton.md §6）：
// - `ESCODE_RUST_HOST=1` 时 Main 懒启动一个 App 级 Rust Host（每 App 一个进程，所有窗口/手机远控
//   经 attachment 接入同一个 Host，符合 AGENTS.md 的状态所有者约定）；
// - `attachLocalPort`：给一个 attachment 生产 renderer/relay 侧的 MessagePort——Main 把另一端桥到
//   Rust Host 的 RPC WebSocket，因此**消费方协议不变**（MessagePort 上的 `@escode/rpc`）。
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
  /** 启动握手截止时间（默认 10s，docs/specs/rust-u2-host-resilience.md §4）。 */
  startTimeoutMs?: number;
  /** 日志钩子。`error` 用于启动失败与 Host 的结构化 error 级日志（缺省退回 `warn`）。 */
  log?: {
    info: (...args: unknown[]) => void;
    warn: (...args: unknown[]) => void;
    error?: (...args: unknown[]) => void;
  };
}

export interface RustHostAttachment {
  /** 交给 renderer/relay 的端口（另一端已在 Main 桥到 Rust Host）。 */
  port: unknown;
  /** 解绑：关闭桥与端口。 */
  dispose(): void;
}

export interface RustHostManager {
  /** 开关是否打开（关闭时调用方走 Node Host）。 */
  readonly enabled: boolean;
  /** 懒启动（single-flight）；失败抛带分类的 `RustHostStartError`（spec §4）。 */
  ensureStarted(): Promise<RustHostHandle>;
  /**
   * 生产一个 attachment 端口（内部 `ensureStarted` + 桥接）。`clientMode` 与 Node 路径同一
   * attachment 的交付模式一致（Web 远控共享 Host = `web-remote-replayable`）。
   */
  attachLocalPort(
    createChannel: () => { port1: unknown; port2: unknown },
    clientMode: RustHostClientMode,
  ): Promise<RustHostAttachment>;
  /** 当前是否已有 Host 进程在跑（诊断/测试）。 */
  isRunning(): boolean;
  /** 收口（应用退出）。 */
  stop(): Promise<void>;
}

export function createRustHostManager(options: RustHostManagerOptions): RustHostManager {
  const enabled = options.enabled ?? isRustHostEnabled();
  let handle: RustHostHandle | undefined;
  let starting: Promise<RustHostHandle> | undefined;

  async function ensureStarted(): Promise<RustHostHandle> {
    if (handle) return handle;
    starting ??= startRustHost({
      binary: options.binary,
      ...(options.args ? { args: options.args } : {}),
      home: options.home,
      ...(options.startTimeoutMs ? { timeoutMs: options.startTimeoutMs } : {}),
      ...(options.dataBaseDir ? { dataBaseDir: options.dataBaseDir } : {}),
      ...(options.runtimeEnv ? { runtimeEnv: await options.runtimeEnv() } : {}),
      ...(options.authorizeLocalMediaPreviewPath
        ? { authorizeLocalMediaPreviewPath: options.authorizeLocalMediaPreviewPath }
        : {}),
      // Host 的结构化日志（`logBatch` 控制帧）进同一条 Main 日志出口；scope 作为来源标签。
      onLogBatch: (batch) => {
        for (const line of batch.lines) {
          const text = `[rust-host] (${batch.scope}) ${line}`;
          if (batch.level === "error") (options.log?.error ?? options.log?.warn)?.(text);
          else if (batch.level === "warn") options.log?.warn(text);
          else if (batch.level === "info") options.log?.info(text);
          // `debug`（协议原始数据/逐条更新类高频诊断）生产不落盘，Host 侧目前也不产生，不转发。
        }
      },
    })
      .then((started) => {
        handle = started;
        options.log?.info(`[rust-host] started at ${started.addr}`);
        return started;
      })
      .catch((error: unknown) => {
        starting = undefined;
        // 修复：过去记 warn 并声称“回退 Node Host”，而调用方并不回退、错误直接上抛。
        // 启动失败是不可恢复错误（AGENTS.md 日志级别），按分类记 error 后原样抛出。
        const classified =
          error instanceof RustHostStartError
            ? error
            : new RustHostStartError("RUST_HOST_EXITED_EARLY", String(error));
        (options.log?.error ?? options.log?.warn)?.(
          `[rust-host] start failed (${classified.code}): ${classified.message}`,
        );
        throw classified;
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
      const teardown = bridgeRustHostPort(started, channel.port2 as never, clientMode);
      return {
        port: channel.port1,
        dispose() {
          teardown();
          channel.port1.close?.();
          channel.port2.close?.();
        },
      };
    },
    isRunning: () => handle !== undefined,
    async stop() {
      const started = handle;
      handle = undefined;
      starting = undefined;
      await started?.stop();
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
