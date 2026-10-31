import { spawn } from "node:child_process";
import { WebSocket, type RawData } from "ws";
import { ProtocolMessageType } from "@escode/rpc";
import type {
  HostPortLike,
  RustHostBridgeCloseEvent,
  RustHostBridgeOptions,
  RustHostClientMode,
  RustHostExit,
  RustHostHandle,
  RustHostLogBatch,
  RustHostRemoteAssets,
  RustHostRemoteScope,
  RustHostStartErrorCode,
  RustHostStartStage,
  StartRustHostOptions,
} from "./rustHostBridgeTypes.js";

export { RUST_HOST_CAPABILITY_HEADER, RUST_HOST_SECRET_HEADER } from "./rustHostBridgeHttp.js";
export { cancelRustHostRemoteProfile, resolveRustHostRemoteProfile } from "./rustHostRemoteProfile.js";

export type {
  HostPortLike,
  RustHostBridgeCloseEvent,
  RustHostBridgeOptions,
  RustHostClientMode,
  RustHostExit,
  RustHostHandle,
  RustHostLogBatch,
  RustHostRemoteAssets,
  RustHostRemoteProfileRequest,
  RustHostRemoteScope,
  RustHostResolvedRemoteProfile,
  RustHostStartErrorCode,
  RustHostStartStage,
  StartRustHostOptions,
} from "./rustHostBridgeTypes.js";

// Host 默认走 Rust（显式 `ESCODE_RUST_HOST=0/false` 才回退 Node）：窗口的本地 Host 通路由
// Electron utility process 切到 Rust Host（docs/specs/rust-u2-host-skeleton.md §6）。切换点是 Main 的
// `attachWorkspaceHost` 本地分支：renderer 仍拿到一个 MessagePort（协议层 SocketProtocol
// 不变），Main 把该端口与 Rust Host 的 RPC WebSocket（`@escode/rpc` 协议，二进制帧）
// 字节级桥接——Renderer 代码不变。
//
// 端口形状对 Electron `MessagePortMain` 与 Node `worker_threads` MessagePort 都成立
// （同形的最小接口，便于在 vitest 里用真实 Rust Host + 真实 MessagePort 验证这条路径）。

/** 握手阶段：超时与提前退出时指明卡在哪一步。 */
import { answerAuthorize, answerMainRequest } from "./rustHostControlReplies.js";

import { createControlChannel } from "./rustHostControlChannel.js";
import { RUST_HOST_CAPABILITY_HEADER, RUST_HOST_SECRET_HEADER } from "./rustHostBridgeHttp.js";
import { logger } from "./logger.js";
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

/**
 * `ESCODE_RUST_HOST` 默认开启（M5 已翻转，四平台安装包验收见 rust-u2-window-host.md §6）；
 * 显式 `0`/`false` 才回退 Node Host。其它未识别值保持默认开启，避免拼写错误静默降级。
 */
export function isRustHostEnabled(env: NodeJS.ProcessEnv = process.env): boolean {
  const raw = (env.ESCODE_RUST_HOST ?? "").trim().toLowerCase();
  return raw !== "0" && raw !== "false";
}

/**
 * 启动 Rust Host 并完成控制握手：`hostReady` → `rpcListen` → `rpcListening`。
 * 失败一律抛 {@link RustHostStartError}（spec §4 四类），整个握手共用一个截止时间 `timeoutMs`；
 * 二进制缺失与提前退出不等截止时间，事件到达即失败。失败后子进程被 SIGKILL。
 */
export async function startRustHost(options: StartRustHostOptions): Promise<RustHostHandle> {
  const child = spawn(options.binary, options.args ?? [], {
    stdio: ["pipe", "pipe", "inherit"],
    // 修复 Windows 上 Host 被控制台控制事件终止（exit code 0xC000013A = STATUS_CONTROL_C_EXIT）：
    // escode-host 是 console 子系统程序，不隐藏时 Windows 会为它创建控制台窗口；关闭该窗口或
    // Ctrl+C 都会直接杀掉 Host。与仓库内其它 Windows helper 的 spawn 保持一致。
    // stderr 仍走 inherit，Host 结构化日志丢弃时的 stderr 镜像不受影响。
    windowsHide: true,
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
        ...(options.remoteAssets ? { remoteAssets: options.remoteAssets } : {}),
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

/**
 * 向 Rust Host 申请一次性 capability（需进程密钥；Host 拒绝任何带 Origin 的请求）。
 *
 * 带 `scope` 时 Host 先建连再签发；4xx 的错误文本原样上抛（spec §10.12：不重试、不回退 Node Host）。
 */
export async function issueRustHostCapability(
  handle: RustHostHandle,
  scope?: RustHostRemoteScope,
): Promise<string> {
  const response = await fetch(`http://${handle.addr}/api/rpc-host-capability`, {
    method: "POST",
    headers: {
      [RUST_HOST_SECRET_HEADER]: handle.secret,
      ...(scope ? { "content-type": "application/json" } : {}),
    },
    ...(scope ? { body: JSON.stringify({ scope }) } : {}),
  });
  if (!response.ok) {
    throw new Error(
      `Rust Host refused to issue ${scope ? "a remote workspace capability" : "a capability"} (HTTP ${response.status})${await capabilityFailureDetail(response)}`,
    );
  }
  const body = (await response.json()) as { capability?: unknown };
  if (typeof body.capability !== "string" || body.capability.length === 0) {
    throw new Error("Rust Host capability response is malformed");
  }
  return body.capability;
}

/** Host 的 `{error}` 体（连接失败原因）→ 追加到错误文案；非 JSON 错误体只保留状态码。 */
async function capabilityFailureDetail(response: Response): Promise<string> {
  try {
    const body = (await response.json()) as { error?: unknown };
    return typeof body.error === "string" && body.error.trim().length > 0
      ? `: ${body.error.trim()}`
      : "";
  } catch {
    return "";
  }
}

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
  scope?: RustHostRemoteScope,
  options?: RustHostBridgeOptions,
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
  void issueRustHostCapability(handle, scope)
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
        options?.onReady?.();
        while (queue.length > 0) {
          const next = queue.shift();
          if (next) opened.send(next);
        }
      });
      opened.on("unexpected-response", (_request, response) => {
        const unexpected = !closed;
        logger.warn(
          `[rust-host] attachment websocket rejected (HTTP ${String(response.statusCode)})`,
        );
        teardown();
        if (unexpected) {
          options?.onUnexpectedClose?.({
            reason: `attachment websocket rejected (HTTP ${String(response.statusCode)})`,
          });
        }
      });
      opened.on("message", (data: RawData) => {
        if (closed) return;
        deliverSocketChunk(toBuffer(data));
      });
      opened.on("close", (code: number, reason: Buffer) => {
        const unexpected = !closed;
        if (unexpected) {
          logger.warn(
            `[rust-host] attachment websocket closed (${String(code)}) ${reason.toString()}`,
          );
        }
        teardown();
        if (unexpected) {
          options?.onUnexpectedClose?.({ code, reason: reason.toString() });
        }
      });
      opened.on("error", (error: Error) => {
        const unexpected = !closed;
        logger.warn("[rust-host] attachment websocket failed", error.message);
        teardown();
        if (unexpected) options?.onUnexpectedClose?.({ reason: error.message });
      });
    })
    .catch((error: unknown) => {
      const unexpected = !closed;
      const message = error instanceof Error ? error.message : String(error);
      logger.warn("[rust-host] attachment capability failed", message);
      teardown();
      // capability 申请失败就是 scope 建连失败（含首次安装失败）：调用方必须收到显式原因，
      // 不能只留下一条日志后让 attachment 永久停在「未 ready」。
      if (unexpected) options?.onUnexpectedClose?.({ reason: message });
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
