import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { WebSocket, type RawData } from "ws";
import { ProtocolMessageType } from "@escode/rpc";

// Host 默认走 Rust（显式 `ESCODE_RUST_HOST=0/false` 才回退 Node）：窗口的本地 Host 通路由
// Electron utility process 切到 Rust Host（docs/specs/rust-u2-host-skeleton.md §6）。切换点是 Main 的
// `attachWorkspaceHost` 本地分支：renderer 仍拿到一个 MessagePort（协议层 SocketProtocol
// 不变），Main 把该端口与 Rust Host 的 RPC WebSocket（`@escode/rpc` 协议，二进制帧）
// 字节级桥接——Renderer 代码不变。
//
// 端口形状对 Electron `MessagePortMain` 与 Node `worker_threads` MessagePort 都成立
// （同形的最小接口，便于在 vitest 里用真实 Rust Host + 真实 MessagePort 验证这条路径）。

/** 与 Electron `MessagePortMain` / Node `MessagePort` 共形的最小接口。 */
export interface HostPortLike {
  on(event: "message", listener: (event: { data?: unknown } | unknown) => void): void;
  postMessage(value: unknown): void;
  start?(): void;
  close?(): void;
}

/**
 * Rust Host 启动失败的分类（docs/specs/rust-u2-host-resilience.md §4）。`RUST_HOST_CRASH_LOOP`
 * 由 `rustHostManager` 在受限重启用完次数后给出（§3.2）。
 */
export type RustHostStartErrorCode =
  | "RUST_HOST_BINARY_MISSING"
  | "RUST_HOST_EXITED_EARLY"
  | "RUST_HOST_HANDSHAKE_TIMEOUT"
  | "RUST_HOST_LISTEN_FAILED"
  | "RUST_HOST_CRASH_LOOP";

/** 握手阶段：超时与提前退出时指明卡在哪一步。 */
import { answerAuthorize, answerMainRequest } from "./rustHostControlReplies.js";

import { createControlChannel } from "./rustHostControlChannel.js";
import { logger } from "./logger.js";
export type RustHostStartStage = "hostReady" | "rpcListening";

/** 带分类的 Rust Host 启动错误（调用方按 `code` 决定呈现，不解析文案）。 */
export class RustHostStartError extends Error {
  override readonly name = "RustHostStartError";
  constructor(
    readonly code: RustHostStartErrorCode,
    message: string,
    readonly details: {
      stage?: RustHostStartStage;
      timeoutMs?: number;
      exitCode?: number | null;
      signal?: NodeJS.Signals | null;
      errno?: string;
    } = {},
  ) {
    super(message);
  }
}

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

/**
 * `ESCODE_RUST_HOST` 默认开启（M5 已翻转，四平台安装包验收见 rust-u2-window-host.md §6）；
 * 显式 `0`/`false` 才回退 Node Host。其它未识别值保持默认开启，避免拼写错误静默降级。
 */
export function isRustHostEnabled(env: NodeJS.ProcessEnv = process.env): boolean {
  const raw = (env.ESCODE_RUST_HOST ?? "").trim().toLowerCase();
  return raw !== "0" && raw !== "false";
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

/** Host 侧一条日志批次（`ControlMessage::LogBatch`）。 */
export interface RustHostLogBatch {
  scope: string;
  level: "debug" | "info" | "warn" | "error";
  lines: string[];
}

/**
 * 启动 Rust Host 并完成控制握手：`hostReady` → `rpcListen` → `rpcListening`。
 * 失败一律抛 {@link RustHostStartError}（spec §4 四类），整个握手共用一个截止时间 `timeoutMs`；
 * 二进制缺失与提前退出不等截止时间，事件到达即失败。失败后子进程被 SIGKILL。
 */
export async function startRustHost(options: StartRustHostOptions): Promise<RustHostHandle> {
  const child = spawn(options.binary, options.args ?? [], {
    stdio: ["pipe", "pipe", "inherit"],
    env: {
      ...process.env,
      ...options.env,
      HOME: options.home,
      USERPROFILE: options.home,
    },
  });
  // 修复：过去没有监听子进程 `error`：二进制缺失时 Node 把未处理的 `error` 事件当异常抛出
  // （Main 进程崩溃），而握手只能等满超时。现在 `error` 与 `exit` 都转成分类失败，事件到达即结束等待。
  let exit: RustHostExit | undefined;
  const exitListeners: Array<(exit: RustHostExit) => void> = [];
  let failStartup: ((error: RustHostStartError) => void) | undefined;
  const startupFailed = new Promise<never>((_resolve, reject) => {
    failStartup = reject;
  });
  // 握手结束后的失败只经 `onExit` 报告；这里挂 no-op，避免成功启动后 Promise 未处理。
  startupFailed.catch(() => undefined);
  let stage: RustHostStartStage = "hostReady";
  child.on("error", (error: NodeJS.ErrnoException) => {
    const errno = error.code ?? "";
    failStartup?.(
      ["ENOENT", "EACCES", "ENOTDIR", "EPERM"].includes(errno)
        ? new RustHostStartError(
            "RUST_HOST_BINARY_MISSING",
            `Rust Host binary cannot be started (${errno}): ${options.binary}`,
            { errno },
          )
        : new RustHostStartError(
            "RUST_HOST_EXITED_EARLY",
            `Rust Host failed to start: ${error.message}`,
            { stage, errno },
          ),
    );
  });
  child.once("exit", (code, signal) => {
    exit = { code, signal };
    failStartup?.(
      new RustHostStartError(
        "RUST_HOST_EXITED_EARLY",
        `Rust Host exited before ${stage} (code ${String(code)}, signal ${String(signal)})`,
        { stage, exitCode: code, signal },
      ),
    );
    for (const listener of exitListeners.splice(0)) listener(exit);
  });
  const control = createControlChannel(child, {
    onLogBatch: options.onLogBatch,
    onAuthorizeRequest: options.authorizeLocalMediaPreviewPath
      ? (request) => answerAuthorize(child, request, options.authorizeLocalMediaPreviewPath!)
      : undefined,
    onMainRequest: options.onMainRequest
      ? (request) => answerMainRequest(child, request, options.onMainRequest!)
      : undefined,
    onHostEvent: options.onHostEvent,
    onWorkspaceStartupState: options.onWorkspaceStartupState,
  });
  const timeoutMs = options.timeoutMs ?? 10_000;
  let deadline: ReturnType<typeof setTimeout> | undefined;
  const timedOut = new Promise<never>((_resolve, reject) => {
    deadline = setTimeout(
      () =>
        reject(
          new RustHostStartError(
            "RUST_HOST_HANDSHAKE_TIMEOUT",
            `Rust Host did not report ${stage} within ${timeoutMs}ms`,
            { stage, timeoutMs },
          ),
        ),
      timeoutMs,
    );
  });
  const step = <T>(promise: Promise<T>) => Promise.race([promise, startupFailed, timedOut]);
  try {
    await step(control.next());
    stage = "rpcListening";
    child.stdin.write(
      `${JSON.stringify({
        kind: "rpcListen",
        bind: options.bind ?? "127.0.0.1:0",
        home: options.home,
        ...(options.dataBaseDir ? { dataBaseDir: options.dataBaseDir } : {}),
        ...(options.runtimeEnv ? { runtimeEnv: options.runtimeEnv } : {}),
        ...(options.authorizeLocalMediaPreviewPath ? { localMediaPreview: true } : {}),
        ...(options.onMainRequest ? { mainServices: true } : {}),
      })}\n`,
    );
    const listening = await step(
      control.nextMatching(
        (message) => message.kind === "rpcListening" || message.kind === "failed",
      ),
    );
    if (listening.kind === "failed") {
      throw new RustHostStartError(
        "RUST_HOST_LISTEN_FAILED",
        `Rust Host failed to listen: ${JSON.stringify(listening.error ?? listening)}`,
        { stage },
      );
    }
    const addr = `127.0.0.1:${String(listening.addr).replace(/^.*:/, "")}`;
    if (typeof listening.secret !== "string" || listening.secret.length === 0) {
      throw new RustHostStartError(
        "RUST_HOST_LISTEN_FAILED",
        "Rust Host did not report an RPC secret",
        { stage },
      );
    }
    const secret = listening.secret;
    // 握手完成：此后的退出只经 onExit 报告给所有者（rustHostManager），不再是启动失败。
    failStartup = undefined;
    return {
      child,
      pid: child.pid,
      addr,
      secret,
      onExit(listener) {
        if (exit) listener(exit);
        else exitListeners.push(listener);
      },
      send(message) {
        if (exit) return;
        child.stdin.write(`${JSON.stringify(message)}\n`);
      },
      async stop() {
        if (exit) return;
        child.stdin.end();
        await withTimeout(
          new Promise<void>((done) => child.once("close", () => done())),
          5_000,
          "Rust Host did not exit",
        ).catch(() => child.kill("SIGKILL"));
      },
    };
  } catch (error) {
    failStartup = undefined;
    child.kill("SIGKILL");
    throw error;
  } finally {
    if (deadline) clearTimeout(deadline);
  }
}

/** 向 Rust Host 申请一次性 capability（需进程密钥；Host 拒绝任何带 Origin 的请求）。 */
export async function issueRustHostCapability(handle: RustHostHandle): Promise<string> {
  const response = await fetch(`http://${handle.addr}/api/rpc-host-capability`, {
    method: "POST",
    headers: { [RUST_HOST_SECRET_HEADER]: handle.secret },
  });
  if (!response.ok) {
    throw new Error(`Rust Host refused to issue a capability (HTTP ${response.status})`);
  }
  const body = (await response.json()) as { capability?: unknown };
  if (typeof body.capability !== "string" || body.capability.length === 0) {
    throw new Error("Rust Host capability response is malformed");
  }
  return body.capability;
}

/** 进程密钥头（与 Rust `http::HOST_SECRET_HEADER` 同名）。 */
export const RUST_HOST_SECRET_HEADER = "x-escode-host-secret";
/** capability 头（TS `ESCODE_RPC_HOST_CAPABILITY_HEADER` 同名）。 */
export const RUST_HOST_CAPABILITY_HEADER = "x-escode-rpc-host-capability";

/** attachment 的交付模式（决定连接路径；Rust Host 按路径赋予可信 clientMode）。 */
export type RustHostClientMode = "desktop-continuous" | "web-remote-replayable";

/**
 * 把一个 attachment 的 MessagePort 桥到 Rust Host 的 RPC WebSocket。返回解绑函数。
 * 路径决定交付模式：`web-remote-replayable` → `/ws`，`desktop-continuous` → `/ws/host`；
 * 两者都要一次性 capability（docs/specs/rust-u2-agent-v4.md §3.2）。两个方向都做字节透传。
 *
 * 修复（评审）：A9 访问控制改动曾把桥接写死为 `/ws/host`（desktop-continuous）；而远程 workspace
 * attachment（`desktopRemoteSessions.ts` 的 `attachRemoteWorkspaceSessionHost`）使用的是
 * `web-remote-replayable`。写死 continuous 会让远程 workspace 拿到桌面投递语义（不可回放、
 * desktop-only rows）。现在由调用方显式声明模式。
 * capability 申请期间到达的端口消息按序排队，连上后原样补发。
 */
export function bridgeRustHostPort(
  handle: RustHostHandle,
  port: HostPortLike,
  clientMode: RustHostClientMode,
): () => void {
  // Rust Host 的 RPC WebSocket 使用 `@escode/rpc` 的 SocketProtocol 帧
  // （13 字节头：type/id/ack/length），而窗口 MessagePort 使用无帧的
  // MessagePortProtocol。这里必须转帧；直接字节透传会让 renderer 把帧头
  // 当成消息内容，ChannelClient 永远收不到 Initialize。
  const PROTOCOL_HEADER_SIZE = 13;
  const queue: Buffer[] = [];
  let inbound = Buffer.alloc(0);
  let closed = false;
  let socket: WebSocket | null = null;

  const frameForSocket = (payload: Buffer): Buffer => {
    const frame = Buffer.allocUnsafe(PROTOCOL_HEADER_SIZE + payload.length);
    frame.writeUInt8(ProtocolMessageType.Regular, 0);
    frame.writeUInt32BE(0, 1);
    frame.writeUInt32BE(0, 5);
    frame.writeUInt32BE(payload.length, 9);
    payload.copy(frame, PROTOCOL_HEADER_SIZE);
    return frame;
  };

  const deliverSocketChunk = (chunk: Buffer): void => {
    inbound = inbound.length === 0 ? chunk : Buffer.concat([inbound, chunk]);
    while (inbound.length >= PROTOCOL_HEADER_SIZE) {
      const type = inbound.readUInt8(0);
      const length = inbound.readUInt32BE(9);
      const total = PROTOCOL_HEADER_SIZE + length;
      // body 未到齐时不能提前消费 header（与 SocketProtocol.readMessages 同语义）。
      if (inbound.length < total) return;
      const payload = inbound.subarray(PROTOCOL_HEADER_SIZE, total);
      inbound = inbound.subarray(total);
      if (type === ProtocolMessageType.Regular) {
        port.postMessage(length === 0 ? Buffer.alloc(0) : Buffer.from(payload));
      }
    }
  };

  const teardown = () => {
    closed = true;
    if (
      socket &&
      (socket.readyState === WebSocket.OPEN || socket.readyState === WebSocket.CONNECTING)
    ) {
      socket.close();
    }
  };
  port.on("message", (event: { data?: unknown } | unknown) => {
    if (closed) return;
    const payload = toBuffer(extractPortData(event));
    const frame = frameForSocket(payload);
    if (socket?.readyState === WebSocket.OPEN) socket.send(frame);
    else queue.push(frame);
  });
  port.start?.();
  void issueRustHostCapability(handle)
    .then((capability) => {
      if (closed) return;
      const path = clientMode === "desktop-continuous" ? "/ws/host" : "/ws";
      const opened = new WebSocket(`ws://${handle.addr}${path}`, {
        headers: { [RUST_HOST_CAPABILITY_HEADER]: capability },
      });
      socket = opened;
      opened.binaryType = "nodebuffer";
      opened.on("open", () => {
        logger.info(`[rust-host] attachment websocket open (${path})`);
        while (queue.length > 0) {
          const next = queue.shift();
          if (next) opened.send(next);
        }
      });
      opened.on("unexpected-response", (_request, response) => {
        logger.warn(
          `[rust-host] attachment websocket rejected (HTTP ${String(response.statusCode)})`,
        );
        teardown();
      });
      opened.on("message", (data: RawData) => {
        if (closed) return;
        deliverSocketChunk(toBuffer(data));
      });
      opened.on("close", (code: number, reason: Buffer) => {
        if (!closed) {
          logger.warn(
            `[rust-host] attachment websocket closed (${String(code)}) ${reason.toString()}`,
          );
        }
        teardown();
      });
      opened.on("error", (error: Error) => {
        logger.warn("[rust-host] attachment websocket failed", error.message);
        teardown();
      });
    })
    .catch((error: unknown) => {
      logger.warn(
        "[rust-host] attachment capability failed",
        error instanceof Error ? error.message : error,
      );
      teardown();
    });
  return teardown;
}

function extractPortData(event: { data?: unknown } | unknown): unknown {
  if (event && typeof event === "object" && "data" in event) {
    return (event as { data?: unknown }).data;
  }
  return event;
}

function toBuffer(value: unknown): Buffer {
  if (Buffer.isBuffer(value)) return value;
  if (value instanceof ArrayBuffer) return Buffer.from(new Uint8Array(value));
  if (ArrayBuffer.isView(value)) {
    return Buffer.from(value.buffer, value.byteOffset, value.byteLength);
  }
  if (Array.isArray(value)) return Buffer.concat(value.map(toBuffer));
  throw new Error("Rust Host bridge received a non-binary port message");
}

async function withTimeout<T>(promise: Promise<T>, ms: number, message: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      promise,
      new Promise<never>((_resolve, reject) => {
        timer = setTimeout(() => reject(new Error(message)), ms);
      }),
    ]);
  } finally {
    if (timer) clearTimeout(timer);
  }
}
