import type { RustHostStartError } from "./rustHostBridge.js";
import type {
  RustHostClientMode,
  RustHostExit,
  RustHostHandle,
  RustHostRemoteAssets,
  RustHostRemoteProfileRequest,
  RustHostRemoteScope,
  RustHostResolvedRemoteProfile,
} from "./rustHostBridgeTypes.js";

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
  /** Rust Host 模式的一次性存储准备；在 spawn Host 前完成，失败以 `RUST_HOST_STORAGE_FAILED` 拒绝。 */
  prepareStorage?: () => Promise<void>;
  /** Source Server 首次安装资源（spec §11.3）；懒求值，避免启动期读取。 */
  remoteAssets?: () => RustHostRemoteAssets;
  /** 本地媒体路径授权（Main 的注册表）；提供后 Host 的 media-preview 回 `local-url`（见 rustHostBridge）。 */
  authorizeLocalMediaPreviewPath?: (path: string) => Promise<string>;
  /** Main 专属服务（cua-permission / sandbox 注册）；提供后 Host 经控制通道转发。 */
  onMainRequest?: (request: { method: string; params: unknown }) => Promise<unknown>;
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
  | {
      state: "restarting";
      attempt: number;
      delayMs: number;
      exit: RustHostExit;
    }
  | { state: "failed"; error: RustHostStartError };

export interface RustHostAttachment {
  /** 交给 renderer/relay 的端口（另一端已在 Main 桥到 Rust Host）。 */
  port: unknown;
  /** 解绑：关闭桥与端口。 */
  dispose(): void;
  /**
   * Host 意外退出时回调一次（端口已关闭、桥已拆）。消费方据此释放并按自己的恢复语义重建
   * （spec §3.4：不在原端口上原地重连）。主动 `dispose()` 不触发。
   */
  onLost(listener: (reason: RustHostAttachmentLostReason) => void): void;
  /**
   * 远端 scope 的 `/ws` 被 Host 非主动结束（Source Server 链路 EOF 等）时回调一次。
   * Host 进程退出也伴随 socket 关闭，但该场景由 `onLost` 表达；调用方主动 `dispose()` 不触发。
   */
  onClosed?(listener: (event: RustHostAttachmentClosedEvent) => void): void;
}

export type RustHostAttachmentLostReason = {
  reason: "host-exited";
  exit: RustHostExit;
};

/** 远端 scope 的桥接被非主动关闭（spec §10.14）；与 Host 退出的 `onLost` 分开，避免时序竞态。 */
export interface RustHostAttachmentClosedEvent {
  code?: number;
  reasonText?: string;
}

export interface RustHostManager {
  /** 懒启动（single-flight）；失败抛 `RustHostStartError`。重启退避期间等待同一次重启。 */
  ensureStarted(): Promise<RustHostHandle>;
  /**
   * 生产一个 attachment 端口（内部 `ensureStarted` + 桥接）。`clientMode` 与 Node 路径同一
   * attachment 的交付模式一致（远程 workspace共享 Host = `web-remote-replayable`）。
   */
  attachLocalPort(
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
  ): Promise<RustHostAttachment>;
  /**
   * 远端 workspace 的 attachment 端口（S3-c，spec §10.12）：Main 用 `scope` 申请 scoped capability，
   * Host 建立/复用该 profile 的连接，再把 RPC WebSocket 桥回 MessagePort。失败（profile 未知、
   * transport/握手失败）显式抛出，不回退 Node Host。
   *
   * `clientMode` 必填：桌面远端窗口是 `desktop-continuous`（`/ws/host`），手机/Web 共享 Host
   * attachment 是 `web-remote-replayable`（`/ws`，spec §10.15）；不能按进程或 transport 猜测。
   */
  attachRemoteScopePort(
    createChannel: () => { port1: unknown; port2: unknown },
    scope: RustHostRemoteScope,
    clientMode: RustHostClientMode,
  ): Promise<RustHostAttachment>;
  /** 首连解析：Host 匹配/创建 Profile、连接并回填 sourceServerId（spec §12.2）。 */
  resolveRemoteProfile(
    request: RustHostRemoteProfileRequest,
    options?: { signal?: AbortSignal },
  ): Promise<RustHostResolvedRemoteProfile>;
  /** 首连取消：按 Host 侧 caller key 丢弃在建 transport（spec §12.2）；Host 未启动时无操作。 */
  cancelRemoteProfile(requestId: string): Promise<void>;
  /** 发一条控制消息（workspace retry / 采样 / 网络捕获）。 */
  sendControl(message: Record<string, unknown>): void;
  /** Host 本地的 Host 事实（资源采样、网络捕获、agent 生命周期、CUA 状态）。 */
  onHostEvent(listener: (event: unknown) => void): () => void;
  /** 实例真实 `startup/storageState`。 */
  onWorkspaceStartupState(
    listener: (state: {
      startupId: string;
      windowId: string;
      workspaceKey: string;
      generation: number;
      state: unknown;
    }) => void,
  ): () => void;
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
