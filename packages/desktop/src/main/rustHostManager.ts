import { join } from "node:path";
import { bridgeRustHostPort, isRustHostEnabled, startRustHost, type RustHostHandle } from "./rustHostBridge.js";

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
  /** 设置域根目录（Main 决定）。 */
  home: string;
  /** 显式覆盖开关（默认读 `ESCODE_RUST_HOST`）。 */
  enabled?: boolean;
  /** 日志钩子。 */
  log?: { info: (...args: unknown[]) => void; warn: (...args: unknown[]) => void };
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
  /** 懒启动（single-flight）；失败抛错，调用方回退 Node Host。 */
  ensureStarted(): Promise<RustHostHandle>;
  /** 生产一个 attachment 端口（内部 `ensureStarted` + 桥接）。 */
  attachLocalPort(createChannel: () => { port1: unknown; port2: unknown }): Promise<RustHostAttachment>;
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
    starting ??= startRustHost({ binary: options.binary, home: options.home })
      .then((started) => {
        handle = started;
        options.log?.info(`[rust-host] started at ${started.addr}`);
        return started;
      })
      .catch((error: unknown) => {
        starting = undefined;
        options.log?.warn(`[rust-host] start failed, falling back to Node Host: ${String(error)}`);
        throw error;
      });
    return starting;
  }

  return {
    enabled,
    ensureStarted,
    async attachLocalPort(createChannel) {
      const started = await ensureStarted();
      const channel = createChannel() as {
        port1: { on?: HostPortLikeOn; postMessage(value: unknown): void; start?(): void; close?(): void };
        port2: { on?: HostPortLikeOn; postMessage(value: unknown): void; start?(): void; close?(): void };
      };
      const teardown = bridgeRustHostPort(started, channel.port2 as never);
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
