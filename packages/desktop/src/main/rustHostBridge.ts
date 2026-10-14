import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { WebSocket, type RawData } from "ws";

// dev 开关：`ESCODE_RUST_HOST=1` 时窗口的本地 Host 通路由 Electron utility process
// 切到 Rust Host（docs/specs/rust-u2-host-skeleton.md §6）。切换点是 Main 的
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

/** Rust Host 进程句柄（控制通道 + RPC 端点 + 收口）。 */
export interface RustHostHandle {
  /** RPC 端点地址（`ws://127.0.0.1:<port>` 与 `http://127.0.0.1:<port>` 同端口）。 */
  readonly addr: string;
  /** 收口：先关控制通道（Host 收口实例与传输面），再必要时杀进程。 */
  stop(): Promise<void>;
}

/** `ESCODE_RUST_HOST` 是否为真（`1`/`true`；`0`/`false`/空一律关闭）。 */
export function isRustHostEnabled(env: NodeJS.ProcessEnv = process.env): boolean {
  const raw = (env.ESCODE_RUST_HOST ?? "").trim().toLowerCase();
  return raw === "1" || raw === "true";
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
  /** 追加/覆盖的环境变量。 */
  env?: NodeJS.ProcessEnv;
  /** 启动超时（默认 10s）。 */
  timeoutMs?: number;
}

/**
 * 启动 Rust Host 并完成控制握手：`hostReady` → `rpcListen` → `rpcListening`。
 * 失败（二进制缺失、握手超时、监听失败）一律抛错，调用方据此回退 Node Host。
 */
export async function startRustHost(options: StartRustHostOptions): Promise<RustHostHandle> {
  const child = spawn(options.binary, [], {
    stdio: ["pipe", "pipe", "inherit"],
    env: { ...process.env, ...options.env, HOME: options.home, USERPROFILE: options.home },
  });
  const control = createControlChannel(child);
  const timeoutMs = options.timeoutMs ?? 10_000;
  try {
    await withTimeout(control.next(), timeoutMs, "Rust Host did not report hostReady");
    child.stdin.write(
      `${JSON.stringify({
        kind: "rpcListen",
        bind: "127.0.0.1:0",
        home: options.home,
        ...(options.dataBaseDir ? { dataBaseDir: options.dataBaseDir } : {}),
      })}\n`,
    );
    const listening = await withTimeout(
      control.nextMatching(
        (message) => message.kind === "rpcListening" || message.kind === "failed",
      ),
      timeoutMs,
      "Rust Host did not report rpcListening",
    );
    if (listening.kind === "failed") {
      throw new Error(
        `Rust Host failed to listen: ${JSON.stringify(listening.error ?? listening)}`,
      );
    }
    const addr = `127.0.0.1:${String(listening.addr).replace(/^.*:/, "")}`;
    return {
      addr,
      async stop() {
        child.stdin.end();
        await withTimeout(
          new Promise<void>((done) => child.once("close", () => done())),
          5_000,
          "Rust Host did not exit",
        ).catch(() => child.kill("SIGKILL"));
      },
    };
  } catch (error) {
    child.kill("SIGKILL");
    throw error;
  }
}

/**
 * 把 renderer 的 MessagePort 桥到 Rust Host 的 RPC WebSocket（`/ws`：回环免鉴权，
 * `web-remote-replayable` 语义）。返回解绑函数。两个方向都做字节透传：
 * 端口消息 → WS 二进制帧；WS 二进制帧 → 端口消息。
 */
export function bridgeRustHostPort(handle: RustHostHandle, port: HostPortLike): () => void {
  const socket = new WebSocket(`ws://${handle.addr}/ws`);
  socket.binaryType = "nodebuffer";
  const queue: Buffer[] = [];
  let closed = false;

  socket.on("open", () => {
    while (queue.length > 0) {
      const next = queue.shift();
      if (next) socket.send(next);
    }
  });
  socket.on("message", (data: RawData) => {
    if (closed) return;
    port.postMessage(toBuffer(data));
  });
  const teardown = () => {
    closed = true;
    if (socket.readyState === WebSocket.OPEN || socket.readyState === WebSocket.CONNECTING) {
      socket.close();
    }
  };
  socket.on("close", teardown);
  socket.on("error", teardown);
  port.on("message", (event: { data?: unknown } | unknown) => {
    if (closed) return;
    const payload = toBuffer(extractPortData(event));
    if (socket.readyState === WebSocket.OPEN) socket.send(payload);
    else if (socket.readyState === WebSocket.CONNECTING) queue.push(payload);
  });
  port.start?.();
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

function createControlChannel(child: ChildProcessWithoutNullStreams) {
  let buffer = "";
  const pending: Array<(message: Record<string, unknown>) => void> = [];
  const queue: Array<Record<string, unknown>> = [];
  child.stdout.setEncoding("utf8");
  child.stdout.on("data", (chunk: string) => {
    buffer += chunk;
    let newline: number;
    while ((newline = buffer.indexOf("\n")) >= 0) {
      const line = buffer.slice(0, newline);
      buffer = buffer.slice(newline + 1);
      if (!line.trim()) continue;
      const message = JSON.parse(line) as Record<string, unknown>;
      const resolveNext = pending.shift();
      if (resolveNext) resolveNext(message);
      else queue.push(message);
    }
  });
  const next = () =>
    new Promise<Record<string, unknown>>((resolveNext) => {
      const queued = queue.shift();
      if (queued) resolveNext(queued);
      else pending.push(resolveNext);
    });
  return {
    next,
    async nextMatching(predicate: (message: Record<string, unknown>) => boolean) {
      for (let i = 0; i < 50; i += 1) {
        const message = await next();
        if (predicate(message)) return message;
      }
      throw new Error("Rust Host control stream did not yield the expected frame");
    },
  };
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
