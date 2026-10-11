import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { createConnection } from "node:net";
import { test } from "node:test";
import {
  SERIAL_BROKER_SOCKET_ENV,
  SERIAL_BROKER_TOKEN_ENV,
  ESCODE_HOST_SERIAL_ENV,
  escodeSerialCancelMethod,
  escodeSerialMethods,
} from "@escode/shared/serial";
import {
  createSerialBroker,
  injectSerialBroker,
  type SerialControlPort,
} from "../src/app/serial-broker.js";
import {
  isHostSerialAvailable,
  resolveBuiltInSerialMcpServers,
} from "../src/app/built-in-serial.js";
import { OFFICIAL_SERIAL_PLUGIN_ID } from "../src/app/official-plugin-definitions.js";
import { createProtocolSerialControlPort } from "../src/escode-protocol/serial-control-broker.js";
import { ProtocolRequestError } from "../src/escode-protocol/server-types.js";

const logger = {
  debug() {},
  info() {},
  warn() {},
  error() {},
  child() {
    return logger;
  },
} as never;

async function send(socketPath: string, payload: unknown): Promise<unknown> {
  return await new Promise((resolve, reject) => {
    let buffer = "";
    const socket = createConnection(socketPath);
    socket.once("connect", () => socket.write(`${JSON.stringify(payload)}\n`));
    socket.on("data", (chunk) => {
      buffer += chunk.toString("utf8");
      const newline = buffer.indexOf("\n");
      if (newline >= 0) {
        socket.destroy();
        resolve(JSON.parse(buffer.slice(0, newline)));
      }
    });
    socket.once("error", reject);
  });
}

function request(token: string, extra: Record<string, unknown>) {
  return {
    id: randomUUID(),
    token,
    runtimeScope: "main",
    sessionId: "session-a",
    ...extra,
  };
}

// --- broker ---------------------------------------------------------------------

test("broker 校验 token 后把参数（含默认值）交给端口，并原样返回结果", async () => {
  const calls: unknown[] = [];
  const port: SerialControlPort = {
    request: async (op, args, context) => {
      calls.push({ op, args, sessionId: context.sessionId });
      return { bytes: 2, seq: 7 };
    },
  };
  const broker = createSerialBroker({ port, logger });
  await broker.ready;
  try {
    const payload = request(broker.token, { op: "write", args: { data: "AT" } });
    const response = await send(broker.socketPath, payload);
    assert.deepEqual(response, { id: payload.id, ok: true, result: { bytes: 2, seq: 7 } });
    assert.deepEqual(calls, [
      {
        op: "write",
        args: { data: "AT", encoding: "utf-8", lineEnding: "none" },
        sessionId: "session-a",
      },
    ]);
  } finally {
    await broker.close();
  }
});

test("broker 拒绝错误 token 与子 agent，错误带 code", async () => {
  const port: SerialControlPort = { request: async () => ({}) };
  const broker = createSerialBroker({ port, logger });
  await broker.ready;
  try {
    const wrongToken = await send(
      broker.socketPath,
      request("b".repeat(64), { op: "list", args: {} }),
    );
    assert.equal((wrongToken as { ok: boolean }).ok, false);
    assert.equal((wrongToken as { error: { code: string } }).error.code, "unauthorized");
    const subagent = await send(
      broker.socketPath,
      request(broker.token, { op: "list", args: {}, runtimeScope: "subagent" }),
    );
    assert.equal((subagent as { error: { code: string } }).error.code, "unavailable");
  } finally {
    await broker.close();
  }
});

test("broker 把 Host 的工具错误码透传给 MCP server", async () => {
  const port: SerialControlPort = {
    request: async () => {
      throw new ProtocolRequestError(-32010, "Serial port is not open", { code: "notOpen" });
    },
  };
  const broker = createSerialBroker({ port, logger });
  await broker.ready;
  try {
    const response = await send(
      broker.socketPath,
      request(broker.token, { op: "write", args: { data: "AT" } }),
    );
    assert.deepEqual((response as { error: unknown }).error, {
      code: "notOpen",
      message: "Serial port is not open",
    });
  } finally {
    await broker.close();
  }
});

test("injectSerialBroker 只向 serial server 定向注入连接材料", () => {
  const servers = injectSerialBroker(
    {
      serial: { type: "stdio", command: "node", args: [], env: { A: "1" } },
      other: { type: "stdio", command: "node", args: [] },
    } as never,
    { socketPath: "pipe-x", token: "t".repeat(64) } as never,
  );
  assert.deepEqual((servers.serial as { env: unknown }).env, {
    A: "1",
    [SERIAL_BROKER_SOCKET_ENV]: "pipe-x",
    [SERIAL_BROKER_TOKEN_ENV]: "t".repeat(64),
  });
  assert.equal((servers.other as { env?: unknown }).env, undefined);
});

// --- 协议端口 -------------------------------------------------------------------

function createContext(workspace: Record<string, string | undefined>) {
  const requests: Array<{ method: string; params: Record<string, unknown> }> = [];
  let respond: (value: unknown) => void = () => {};
  const context = {
    sessions: new Map([["session-a", { workspace }]]),
    deps: {},
    requestClient: (method: string, params: Record<string, unknown>) => {
      requests.push({ method, params });
      if (method === escodeSerialMethods.waitFor) {
        return new Promise((resolve) => {
          respond = resolve;
        });
      }
      return Promise.resolve({});
    },
  };
  return { context: context as never, requests, release: () => respond({}) };
}

test("端口按会话补齐 workspace 身份并调用对应反向方法", async () => {
  const { context, requests } = createContext({
    workspacePath: "C:/work",
    workspaceIdentity: "identity-1",
  });
  const port = createProtocolSerialControlPort(context);
  await port.request("list", {}, { sessionId: "session-a" });
  assert.equal(requests[0]?.method, escodeSerialMethods.list);
  assert.equal(requests[0]?.params.workspaceKey, "identity-1");
  assert.equal(requests[0]?.params.workspacePath, "C:/work");
  assert.deepEqual(requests[0]?.params.args, {});
  assert.equal(typeof requests[0]?.params.requestId, "string");
});

test("端口在会话不存在时拒绝，不向 Host 发请求", async () => {
  const { context, requests } = createContext({ workspacePath: "C:/work" });
  const port = createProtocolSerialControlPort(context);
  await assert.rejects(port.request("list", {}, { sessionId: "missing" }));
  assert.equal(requests.length, 0);
});

test("waitFor 被取消时向 Host 发送 serialCancel", async () => {
  const { context, requests, release } = createContext({ workspacePath: "C:/work" });
  const port = createProtocolSerialControlPort(context);
  const controller = new AbortController();
  const waiting = port.request(
    "waitFor",
    { pattern: "READY", timeoutMs: 1000, encoding: "utf-8" },
    { sessionId: "session-a", signal: controller.signal },
  );
  controller.abort();
  release();
  await waiting.catch(() => undefined);
  const cancel = requests.find((item) => item.method === escodeSerialCancelMethod);
  assert.deepEqual(cancel?.params, {
    sessionId: "session-a",
    targetRequestId: requests[0]?.params.requestId,
  });
});

// --- 门控 -----------------------------------------------------------------------

const pluginOutcome = (enabled: boolean) => ({
  plugins: [{ id: OFFICIAL_SERIAL_PLUGIN_ID, enabled, rootPath: "/plugins/serial" }],
});

test("Host 能力标记只认 ESCODE_HOST_SERIAL=1", () => {
  assert.equal(isHostSerialAvailable({ [ESCODE_HOST_SERIAL_ENV]: "1" }), true);
  assert.equal(isHostSerialAvailable({ [ESCODE_HOST_SERIAL_ENV]: "0" }), false);
  assert.equal(isHostSerialAvailable({}), false);
});

test("仅在拿到 broker 且插件启用时注册 serial MCP server", () => {
  const resolve = (brokerAvailable: boolean, enabled: boolean) =>
    resolveBuiltInSerialMcpServers({
      brokerAvailable,
      pluginOutcome: pluginOutcome(enabled) as never,
      workingDirectory: "/work",
    });
  const registered = resolve(true, true).serial;
  assert.equal(registered?.type, "stdio");
  assert.equal((registered as { isolation?: string }).isolation, "workspace");
  // serial server 拒绝旧版协商，注册必须固定现代协议版本（链路测试发现漏写会导致连不上）。
  assert.equal((registered as { protocolVersion?: string }).protocolVersion, "2026-07-28");
  assert.equal(Object.keys(resolve(false, true)).length, 0);
  assert.equal(Object.keys(resolve(true, false)).length, 0);
});

// --- 只读工具默认放行 -----------------------------------------------------------

test("官方插件默认放行只对已注册的本插件宿主 MCP server 生效", async () => {
  const { resolveOfficialPluginDefaultAllowedTools } = await import(
    "../src/app/official-plugin-definitions.js"
  );
  assert.deepEqual(
    resolveOfficialPluginDefaultAllowedTools({
      enabledPluginIds: new Set([OFFICIAL_SERIAL_PLUGIN_ID]),
      registeredMcpServerNames: new Set(["serial"]),
    }).sort(),
    ["mcp__serial__serial_list", "mcp__serial__serial_read", "mcp__serial__serial_wait_for"],
  );
  // 插件关闭或 server 未注册（远程 workspace 等）时不放行任何工具
  assert.deepEqual(
    resolveOfficialPluginDefaultAllowedTools({
      enabledPluginIds: new Set(),
      registeredMcpServerNames: new Set(["serial"]),
    }),
    [],
  );
  assert.deepEqual(
    resolveOfficialPluginDefaultAllowedTools({
      enabledPluginIds: new Set([OFFICIAL_SERIAL_PLUGIN_ID]),
      registeredMcpServerNames: new Set(["node_repl"]),
    }),
    [],
  );
});
