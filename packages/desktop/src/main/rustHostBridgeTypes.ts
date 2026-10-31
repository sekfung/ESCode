import type { ChildProcessWithoutNullStreams } from "node:child_process";

/** 与 Electron `MessagePortMain` / Node `MessagePort` 共形的最小接口。 */
export interface HostPortLike {
  on(event: "message", listener: (event: { data?: unknown } | unknown) => void): void;
  postMessage(value: unknown): void;
  start?(): void;
  close?(): void;
}

export type RustHostStartErrorCode =
  | "RUST_HOST_BINARY_MISSING"
  | "RUST_HOST_STORAGE_FAILED"
  | "RUST_HOST_EXITED_EARLY"
  | "RUST_HOST_HANDSHAKE_TIMEOUT"
  | "RUST_HOST_LISTEN_FAILED"
  | "RUST_HOST_CRASH_LOOP";

export type RustHostStartStage = "hostReady" | "rpcListening";

/** Host 进程的退出信息（`onExit`）。 */
export interface RustHostExit {
  code: number | null;
  signal: NodeJS.Signals | null;
}

/** Rust Host 进程句柄（控制通道 + RPC 端点 + 收口）。 */
export interface RustHostHandle {
  /** 子进程（控制通道 stdin 与退出观察；诊断与测试用）。 */
  readonly child: ChildProcessWithoutNullStreams;
  /** 子进程 pid（重启前后不同；诊断与测试用）。 */
  readonly pid: number | undefined;
  /**
   * 进程退出时回调一次（已退出则立即回调）。是否“意外”由调用方判定：
   * `rustHostManager` 以自己是否调用过 `stop()` 为唯一判据（spec §3.1）。
   */
  onExit(listener: (exit: RustHostExit) => void): void;
  /** RPC 端点地址（`ws://127.0.0.1:<port>` 与 `http://127.0.0.1:<port>` 同端口）。 */
  readonly addr: string;
  /**
   * 本次监听的进程密钥（只经 Host 控制通道获得）：签发一次性 capability 时作为
   * `x-escode-host-secret` 头（docs/specs/rust-u2-agent-v4.md §3.2）。
   */
  readonly secret: string;
  /** 收口：先关控制通道（Host 收口实例与传输面），再必要时杀进程。 */
  stop(): Promise<void>;
  /** 发一条控制消息（workspace prepare / 采样 / 网络捕获等）；顺序即写入顺序。 */
  send(message: Record<string, unknown>): void;
}

export interface StartRustHostOptions {
  /** Host 可执行文件路径（安装包内为主进程同级；dev 为 target/debug）。 */
  binary: string;
  /** 设置域根目录（Host 不读进程 env 推断业务事实，由 Main 指定）。 */
  home: string;
  /**
   * 应用数据根（TS `getDataBaseDir()`：设置项 `dataBaseDir` > `ESCODE_DATA_BASE_DIR` > homedir）。
   * 会话工作区 `<dataBaseDir>/.escode/workspace/default` 等路径由它派生；缺省时相关方法回显式缺口。
   */
  dataBaseDir?: string;
  /**
   * Main 为嵌入运行时合成的环境（审计 A16）：实例的 ambient env 只有这一份（外加 workspace 身份），
   * Host 不从自己的进程 env 推断业务事实。缺省时运行时只剩 `--config` 指定的模型。
   */
  runtimeEnv?: Record<string, string>;
  /**
   * Source Server 首次安装资源（spec §11.3）：含 `manifest-<platformArch>.json` 的 release
   * 目录与 CDN 基址。缺省时 Host 只启动已部署 server（旧行为）。
   */
  remoteAssets?: RustHostRemoteAssets;
  /** 追加/覆盖的环境变量（Host 进程自身的 env，不进实例）。 */
  env?: NodeJS.ProcessEnv;
  /** 可执行文件参数（缺省无参数；测试用它以 Node 脚本模拟各类启动失败）。 */
  args?: string[];
  /** RPC 监听地址（缺省 `127.0.0.1:0`；测试用占用端口验证监听失败）。 */
  bind?: string;
  /** 整个启动握手（`hostReady` → `rpcListening`）的截止时间（默认 10s，spec §4）。 */
  timeoutMs?: number;
  /**
   * 结构化日志钩子：Host 的 `logBatch` 控制帧（`host-control/src/logging.rs` 的进程级 sink）。
   * 缺省时这些帧只被跳过——Host 的 stderr 兜底仍会到达 Main（stdio inherit）。
   * Main 把它接到 `hostLogRelay.onStructuredLog`，与 Node Host 的 postMessage 日志同一条出口。
   */
  onLogBatch?: (batch: RustHostLogBatch) => void;
  /**
   * 本地媒体路径授权（rust-u2-attachments.md §5）：授权注册表的所有者是 Main。提供后 `rpcListen`
   * 声明 `localMediaPreview`，Host 的 `media-preview.prepare` 经控制通道请求授权并回 `local-url`；
   * 缺省 = 旧行为（Host 只走内联分支）。抛错即回 `ok:false`，错误文案原样交给 Host。
   */
  authorizeLocalMediaPreviewPath?: (path: string) => Promise<string>;
  /**
   * Main 专属服务（cua-permission / sandbox 注册）。提供后 `rpcListen` 声明 `mainServices`，
   * Host 经控制通道发 `mainRequest`；缺省 = 旧 Main，相关 channel 回显式缺口。
   */
  onMainRequest?: (request: { method: string; params: unknown }) => Promise<unknown>;
  /** Host 本地的 Host 事实（资源采样、网络捕获、agent 生命周期、CUA 状态）。 */
  onHostEvent?: (event: unknown) => void;
  /** 实例真实 `startup/storageState`（WorkspacePrepare 后逐条上报）。 */
  onWorkspaceStartupState?: (state: {
    startupId: string;
    windowId: string;
    workspaceKey: string;
    generation: number;
    state: unknown;
  }) => void;
}

/** Main 交给 Host 的远程资源事实（`rpcListen.remoteAssets`）。 */
export interface RustHostRemoteAssets {
  releaseDir?: string;
  cdnBaseUrls: string[];
  appVersion: string;
}

/** Host 侧一条日志批次（`ControlMessage::LogBatch`）。 */
export interface RustHostLogBatch {
  scope: string;
  level: "debug" | "info" | "warn" | "error";
  lines: string[];
}

/**
 * 远端 workspace scope（docs/specs/source-server-remote-rust-host.md §10.12）：签发 capability 时
 * Rust Host 先按 `profileId` 建立/复用连接，再把这个 scope 绑定到令牌上。
 */
export interface RustHostRemoteScope {
  sourceServerId: string;
  profileId: string;
  workspaceIdentity: string;
}

/** 首连目标解析请求：`profileId` 优先；否则由 Host 按 `target` 匹配/创建 Profile。 */
export interface RustHostRemoteProfileRequest {
  profileId?: string;
  target?: Record<string, unknown>;
  /** caller key：Host 按它登记在途首连，供 `source-server-cancel-profile` 精确取消。 */
  requestId?: string;
}

/** 首连解析结果：`sourceServerId` 已由握手回填，`target` 是脱敏后的持久 Profile 目标。 */
export interface RustHostResolvedRemoteProfile {
  profileId: string;
  sourceServerId: string;
  target: Record<string, unknown>;
}

/** attachment 的交付模式（决定连接路径；Rust Host 按路径赋予可信 clientMode）。 */
export type RustHostClientMode = "desktop-continuous" | "web-remote-replayable";

/** 桥接的非主动关闭原因（调用方 `teardown()` 不产生该事件）。 */
export interface RustHostBridgeCloseEvent {
  code?: number;
  reason?: string;
}

export interface RustHostBridgeOptions {
  /**
   * Host 结束 `/ws` 或传输失败时回调一次；调用方主动 `dispose()/teardown()` 不触发。
   * 远端 scope 用它把「Source Server 链路 EOF」传导为窗口的断连态（spec §10.14）。
   */
  onUnexpectedClose?: (event: RustHostBridgeCloseEvent) => void;
  /**
   * capability 签发成功且 WebSocket 已打开时回调一次。
   * 远端 scope 的 capability 申请会先让 Host 建连（首次连接含安装，spec §11.3），
   * 调用方以该回调作为「scope 已就绪」边界，不把连接/安装耗时算进 renderer attachment 握手。
   */
  onReady?: () => void;
}
