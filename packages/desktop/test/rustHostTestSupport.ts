// Rust Host 真实客户端测试的共享辅助（spawn Host、控制通道、capability + `/ws/host` 连接）。
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { resolve } from "node:path";
import { expect } from "vitest";
import { WebSocket, type RawData } from "ws";
import { Emitter, SocketProtocol, VSBuffer, type ISocket } from "@escode/rpc";
import { connectViaProtocol } from "@escode/client";

export const hostBinary = resolve(
  `apps/escode-host/target/debug/escode-host${process.platform === "win32" ? ".exe" : ""}`,
);

export function wrapNodeWebSocket(socket: WebSocket): ISocket {
  const onData = new Emitter<VSBuffer>();
  const onClose = new Emitter<void>();
  const onEnd = new Emitter<void>();
  socket.on("message", (raw: RawData) => {
    const data = Array.isArray(raw)
      ? Buffer.concat(raw)
      : Buffer.isBuffer(raw)
        ? raw
        : Buffer.from(raw);
    onData.fire(VSBuffer.wrap(new Uint8Array(data)));
  });
  socket.on("close", () => {
    onClose.fire();
    onEnd.fire();
  });
  socket.on("error", () => {
    onClose.fire();
    onEnd.fire();
  });
  return {
    onData: onData.event,
    onClose: onClose.event,
    onEnd: onEnd.event,
    write(buffer) {
      if (socket.readyState === WebSocket.OPEN) socket.send(buffer.buffer);
    },
    end() {
      socket.close();
    },
    drain: async () => {},
    dispose() {
      socket.close();
    },
  };
}

export interface HostProcess {
  child: ChildProcessWithoutNullStreams;
  send(message: Record<string, unknown>): void;
  next(): Promise<Record<string, unknown>>;
  stop(): Promise<void>;
}

export function startHost(home: string, extraEnv: Record<string, string> = {}): HostProcess {
  const child = spawn(hostBinary, [], {
    stdio: ["pipe", "pipe", "inherit"],
    env: { ...process.env, ...extraEnv, HOME: home, USERPROFILE: home },
  });
  let buffer = "";
  const pending: Array<(value: Record<string, unknown>) => void> = [];
  const queue: Record<string, unknown>[] = [];
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
  return {
    child,
    send(message) {
      child.stdin.write(`${JSON.stringify(message)}\n`);
    },
    next() {
      const queued = queue.shift();
      if (queued) return Promise.resolve(queued);
      return new Promise((resolveNext) => pending.push(resolveNext));
    },
    async stop() {
      child.stdin.end();
      await new Promise<void>((done) => child.once("close", () => done()));
    },
  };
}

export async function nextMeaningful(host: HostProcess): Promise<Record<string, unknown>> {
  for (let i = 0; i < 20; i += 1) {
    const message = await host.next();
    if (message.kind !== "logBatch") return message;
  }
  throw new Error("no meaningful control frame");
}

/** 带进程密钥申请一次性 capability。 */
async function issueCapability(addr: string, secret: string): Promise<string> {
  const response = await fetch(`http://${addr}/api/rpc-host-capability`, {
    method: "POST",
    headers: { "x-escode-host-secret": secret },
  });
  expect(response.status).toBe(200);
  const { capability } = (await response.json()) as { capability: string };
  return capability;
}

function createHostSocket(addr: string, capability: string): WebSocket {
  return new WebSocket(`ws://${addr}/ws/host`, {
    headers: { "x-escode-rpc-host-capability": capability },
  });
}

/** 带进程密钥申请一次性 capability，再连 `/ws/host`（docs/specs/rust-u2-agent-v4.md §3.2）。 */
export async function openHostSocket(addr: string, secret: string): Promise<WebSocket> {
  const capability = await issueCapability(addr, secret);
  return await new Promise((resolveSocket, reject) => {
    const socket = createHostSocket(addr, capability);
    socket.once("error", reject);
    socket.once("open", () => resolveSocket(socket));
  });
}

/**
 * 连接并返回 RPC 服务代理。
 *
 * 修复：旧实现先等 `open` 再挂 `message` 监听。Host 在升级响应之后立即推送 `Initialize` 帧，
 * 两者落在同一次 TCP 读里时，`ws` 会在 `open` 的同一个 tick 内发出 `message`——监听器还没挂上，
 * `Initialize` 丢失，客户端永远不发请求（表现为第一个 RPC 永久挂起）。Host 侧并发工作改变调度时序后
 * 这个竞态必现。这里在 `open` 之前就创建协议并订阅，再等待 `open`。
 */
export async function connectRpc(
  addr: string,
  secret: string,
): Promise<ReturnType<typeof connectViaProtocol>> {
  const socket = createHostSocket(addr, await issueCapability(addr, secret));
  const services = connectViaProtocol(new SocketProtocol(wrapNodeWebSocket(socket)));
  await new Promise<void>((resolveOpen, reject) => {
    socket.once("error", reject);
    socket.once("open", () => resolveOpen());
  });
  return services;
}
