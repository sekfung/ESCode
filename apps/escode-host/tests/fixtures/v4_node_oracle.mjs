// Node 侧 V4 oracle（spec §10.16）：直接连同一个 Source Server fixture，再经一个最小的
// `ChannelServer` 转发（等价 per-window Node Host attachment），用 renderer 视角的
// `ChannelClient` 执行与 Rust 远端 scope 完全相同的 V4 场景，输出规范化 JSON。
// Rust Host 经 `/ws/host` 跑同一场景时，必须得到同一份语义帧。
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { VSBuffer } from "../../../../packages/rpc/src/buffer.ts";
import { ChannelClient } from "../../../../packages/rpc/src/channelClient.ts";
import { ChannelServer } from "../../../../packages/rpc/src/channelServer.ts";
import { createQueuePair, SocketProtocol } from "../../../../packages/rpc/src/protocol.ts";

const here = dirname(fileURLToPath(import.meta.url));
const fixturePath = join(here, "rpc_server_fixture.mjs");
const child = spawn(process.execPath, ["--import", "tsx", fixturePath], {
  stdio: ["pipe", "pipe", "inherit"],
  env: process.env,
});

const socket = {
  onData: (listener) => {
    const handler = (chunk) => listener(VSBuffer.wrap(new Uint8Array(chunk)));
    child.stdout.on("data", handler);
    return { dispose: () => child.stdout.off("data", handler) };
  },
  onClose: () => ({ dispose: () => undefined }),
  onEnd: () => ({ dispose: () => undefined }),
  write: (buffer) => {
    child.stdin.write(Buffer.from(buffer.buffer, buffer.byteOffset, buffer.byteLength));
  },
  end: () => child.stdin.end(),
  drain: async () => undefined,
  dispose: () => undefined,
};

function withTimeout(promise, ms, label) {
  let timer;
  const timeout = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error(`${label} timed out after ${ms}ms`)), ms);
    timer.unref?.();
  });
  return Promise.race([promise, timeout]).finally(() => clearTimeout(timer));
}

/** 只比较语义字段：`serverTime` / `connectionId` 是 Source Server 的非确定值。 */
function normalizeHello(hello) {
  const copy = JSON.parse(JSON.stringify(hello));
  delete copy.serverTime;
  delete copy.connectionId;
  return copy;
}

function normalizeFrame(frame) {
  return JSON.parse(JSON.stringify(frame));
}

function sortFrames(frames) {
  return frames.map(normalizeFrame).sort((left, right) => {
    const byKind = String(left.kind ?? "").localeCompare(String(right.kind ?? ""));
    return byKind !== 0 ? byKind : JSON.stringify(left).localeCompare(JSON.stringify(right));
  });
}

async function waitForFrames(frames, expected, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (frames.length < expected) {
    if (Date.now() > deadline) {
      throw new Error(`expected ${expected} frames, got ${frames.length}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

let remoteClient;
let host;
let client;
try {
  remoteClient = new ChannelClient(new SocketProtocol(socket));
  await withTimeout(remoteClient.waitUntilInitialized(), 10_000, "remote Initialize");

  // Node Host attachment：远端 `escode-agent` 经本地 ChannelServer 暴露给 renderer，
  // 与 per-window Node Host 对远端 workspace 的转发方式一致（只转发，不改写）。
  const remoteAgent = remoteClient.getChannel("escode-agent");
  const [hostProtocol, rendererProtocol] = createQueuePair();
  host = new ChannelServer(hostProtocol, "node-host", 1000, true);
  host.registerChannel("escode-agent", {
    call: (_ctx, command, arg) => remoteAgent.call(command, arg),
    listen: (_ctx, event, arg) => remoteAgent.listen(event, arg),
  });
  host.ready();
  client = new ChannelClient(rendererProtocol);
  await withTimeout(client.waitUntilInitialized(), 10_000, "attachment Initialize");
  const channel = client.getChannel("escode-agent");

  const hello = await channel.call("helloConversationV4");
  const frames = [];
  // `listen()` 返回 Event：首个订阅者出现时才发送 EventListen 帧，与 renderer 同序。
  const subscription = channel.listen(
    "onDynamicConversationFrame",
    undefined,
  )((frame) => {
    frames.push(frame);
  });
  const ack = await channel.call("subscribeConversationV4", {
    sessionId: "fixture-session",
    workspacePath: "/remote/workspace",
    workspaceIdentity: "remote:server:33333333-3333-4333-8333-333333333333:/remote/workspace",
  });
  const commandAck = await channel.call("sendConversationCommandV4", {
    workspacePath: "/remote/workspace",
    workspaceIdentity: "remote:server:33333333-3333-4333-8333-333333333333:/remote/workspace",
    envelope: {
      commandId: "cmd-1",
      clientId: "client-1",
      sessionId: "fixture-session",
      type: "sendText",
      payload: { firstInput: { text: "hello" } },
    },
  });
  await waitForFrames(frames, 2, 10_000);
  subscription.dispose();

  const output = {
    mode: process.env.ESCODE_FIXTURE_V4_MODE ?? null,
    hello: normalizeHello(hello),
    ack: JSON.parse(JSON.stringify(ack)),
    frames: sortFrames(frames),
    commandAck: JSON.parse(JSON.stringify(commandAck)),
  };
  await new Promise((resolve) => process.stdout.write(`${JSON.stringify(output)}\n`, resolve));
} catch (error) {
  console.error(error);
  process.exitCode = 1;
} finally {
  client?.dispose();
  host?.dispose();
  remoteClient?.dispose();
  child.kill();
}
