import { randomUUID } from "node:crypto";
import { createConnection } from "node:net";
import { describe, expect, it } from "vitest";
import {
  CIRCUIT_BROKER_SOCKET_ENV,
  CIRCUIT_BROKER_TOKEN_ENV,
  ESCODE_HOST_CIRCUIT_ENV,
  escodeCircuitMethods,
} from "@escode/shared/circuit";
import {
  createCircuitBroker,
  injectCircuitBroker,
  type CircuitBroker,
  type CircuitControlPort,
} from "../src/app/circuit-broker.js";
import {
  isHostCircuitAvailable,
  resolveBuiltInCircuitMcpServers,
} from "../src/app/built-in-circuit.js";
import {
  OFFICIAL_CIRCUIT_PLUGIN_ID,
  resolveOfficialPluginDefaultAllowedTools,
} from "../src/app/official-plugin-definitions.js";
import { createProtocolCircuitControlPort } from "../src/escode-protocol/circuit-control-broker.js";
import { ProtocolRequestError } from "../src/escode-protocol/server-types.js";

/**
 * circuit 内置插件链路（docs/circuit/README.md §5、§3.5）：
 * broker（MCP server ↔ Agent 运行时）、反向协议端口（→ Host）、注册门控与免批准名单。
 */

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
  return { id: randomUUID(), token, runtimeScope: "main", sessionId: "session-a", ...extra };
}

async function withBroker(
  port: CircuitControlPort,
  run: (broker: CircuitBroker) => Promise<void>,
): Promise<void> {
  const broker = createCircuitBroker({ port, logger });
  await broker.ready;
  try {
    await run(broker);
  } finally {
    await broker.close();
  }
}

describe("circuit broker", () => {
  it("校验 token 后把工具参数交给端口，并原样返回工具信封", async () => {
    const calls: unknown[] = [];
    const envelope = { ok: true, summary: { irId: "ir:1" } };
    await withBroker(
      {
        request: async (op, args, context) => {
          calls.push({ op, args, sessionId: context.sessionId });
          return envelope;
        },
      },
      async (broker) => {
        const payload = request(broker.token, { op: "get", args: { irId: "ir:1" } });
        expect(await send(broker.socketPath, payload)).toEqual({
          id: payload.id,
          ok: true,
          result: envelope,
        });
        expect(calls).toEqual([{ op: "get", args: { irId: "ir:1" }, sessionId: "session-a" }]);
      },
    );
  });

  it("拒绝错误 token、子 agent 与不合契约的参数", async () => {
    await withBroker({ request: async () => ({}) }, async (broker) => {
      const wrongToken = await send(
        broker.socketPath,
        request("b".repeat(64), { op: "get", args: { irId: "ir:1" } }),
      );
      expect(wrongToken).toMatchObject({ ok: false, error: { code: "unavailable" } });

      const subagent = await send(
        broker.socketPath,
        request(broker.token, { op: "get", args: { irId: "ir:1" }, runtimeScope: "subagent" }),
      );
      expect(subagent).toMatchObject({ ok: false, error: { code: "unavailable" } });

      // 未注册的 op（G6 的 submitRecognition）在协议层就没有分支：broker 直接拒绝（invalidInput），不转发给 Host。
      const unknownOp = await send(
        broker.socketPath,
        request(broker.token, {
          op: "submitRecognition",
          args: { path: "board.pdf", draft: {} },
        }),
      );
      expect(unknownOp).toMatchObject({ ok: false, error: { code: "invalidInput" } });
    });
  });

  it("分发只读分析 op 到端口（check / compareFirmware 的参数契约与写类一致严格）", async () => {
    const calls: Array<{ op: string; args: unknown }> = [];
    await withBroker(
      {
        request: async (op, args) => {
          calls.push({ op, args });
          return op === "check"
            ? { ok: true, config: { status: "default" }, items: [] }
            : { ok: true, items: [] };
        },
      },
      async (broker) => {
        const checked = await send(
          broker.socketPath,
          request(broker.token, { op: "check", args: { irId: "ir:1", cursor: "opaque-1" } }),
        );
        expect(checked).toMatchObject({
          ok: true,
          result: { ok: true, config: { status: "default" }, items: [] },
        });

        const compared = await send(
          broker.socketPath,
          request(broker.token, {
            op: "compareFirmware",
            args: { irId: "ir:1", files: ["firmware/board.dts"] },
          }),
        );
        expect(compared).toMatchObject({ ok: true, result: { ok: true, items: [] } });
        expect(calls).toEqual([
          { op: "check", args: { irId: "ir:1", cursor: "opaque-1" } },
          { op: "compareFirmware", args: { irId: "ir:1", files: ["firmware/board.dts"] } },
        ]);

        // 分析工具的 args 也按 shared 的请求 schema 校验：空 files 不转发给 Host。
        const invalidArgs = await send(
          broker.socketPath,
          request(broker.token, { op: "compareFirmware", args: { irId: "ir:1", files: [] } }),
        );
        expect(invalidArgs).toMatchObject({ ok: false, error: { code: "invalidInput" } });
        expect(calls).toHaveLength(2);
      },
    );
  });

  it("把 Host 的工具错误码透传给 MCP server", async () => {
    await withBroker(
      {
        request: async () => {
          throw new ProtocolRequestError(-32603, "当前 revision 为 3", { code: "stale_revision" });
        },
      },
      async (broker) => {
        const response = await send(
          broker.socketPath,
          request(broker.token, {
            op: "correct",
            args: {
              irId: "ir:1",
              baseRevision: 2,
              ops: [{ op: "disconnect", componentId: "r1", pin: "1" }],
            },
          }),
        );
        expect(response).toEqual({
          id: expect.any(String),
          ok: false,
          error: { code: "stale_revision", message: "当前 revision 为 3" },
        });
      },
    );
  });

  it("injectCircuitBroker 只向 circuit server 定向注入连接材料", () => {
    const servers = injectCircuitBroker(
      {
        circuit: { type: "stdio", command: "node", args: [], env: { A: "1" } },
        other: { type: "stdio", command: "node", args: [] },
      } as never,
      { socketPath: "pipe-x", token: "t".repeat(64) } as never,
    );
    expect((servers.circuit as { env: unknown }).env).toEqual({
      A: "1",
      [CIRCUIT_BROKER_SOCKET_ENV]: "pipe-x",
      [CIRCUIT_BROKER_TOKEN_ENV]: "t".repeat(64),
    });
    expect((servers.other as { env?: unknown }).env).toBeUndefined();
  });
});

describe("circuit 反向协议端口", () => {
  function createContext(workspace: Record<string, string | undefined>) {
    const requests: Array<{ method: string; params: Record<string, unknown> }> = [];
    const context = {
      sessions: new Map([["session-a", { workspace }]]),
      deps: {},
      requestClient: (
        method: string,
        params: Record<string, unknown>,
        schema: { parse(value: unknown): unknown },
      ) => {
        requests.push({ method, params });
        return Promise.resolve(schema.parse({ ok: true, by: "ref", items: [] }));
      },
    };
    return { context: context as never, requests };
  }

  it("按会话补齐 workspace 身份与 remoteSessionId，并调用对应反向方法", async () => {
    const { context, requests } = createContext({
      workspacePath: "C:/work",
      workspaceIdentity: "identity-1",
      remoteSessionId: "remote-1",
    });
    const port = createProtocolCircuitControlPort(context);
    const result = await port.request("query", { irId: "ir:1", by: "ref", ref: "R1" } as never, {
      sessionId: "session-a",
    });
    expect(result).toEqual({ ok: true, by: "ref", items: [] });
    expect(requests[0]?.method).toBe(escodeCircuitMethods.query);
    expect(requests[0]?.params.workspaceKey).toBe("identity-1");
    expect(requests[0]?.params.workspacePath).toBe("C:/work");
    expect(requests[0]?.params.remoteSessionId).toBe("remote-1");
    expect(requests[0]?.params.args).toEqual({ irId: "ir:1", by: "ref", ref: "R1" });
    expect(typeof requests[0]?.params.requestId).toBe("string");
  });

  it("会话不存在时拒绝，不向 Host 发请求", async () => {
    const { context, requests } = createContext({ workspacePath: "C:/work" });
    const port = createProtocolCircuitControlPort(context);
    await expect(
      port.request("get", { irId: "ir:1" } as never, { sessionId: "missing" }),
    ).rejects.toThrow(/not active/u);
    expect(requests).toHaveLength(0);
  });

  it("分析 op 走 interaction/circuitCompareFirmware，args 与 cursor 原样带给 Host", async () => {
    const requests: Array<{ method: string; params: Record<string, unknown> }> = [];
    const context = {
      sessions: new Map([["session-a", { workspace: { workspacePath: "C:/work" } }]]),
      deps: {},
      requestClient: (
        method: string,
        params: Record<string, unknown>,
        schema: { parse(value: unknown): unknown },
      ) => {
        requests.push({ method, params });
        return Promise.resolve(schema.parse({ ok: true, items: [], nextCursor: "opaque-2" }));
      },
    };
    const port = createProtocolCircuitControlPort(context as never);
    const result = await port.request(
      "compareFirmware",
      { irId: "ir:1", files: ["firmware/board.dts"], cursor: "opaque-1" } as never,
      { sessionId: "session-a" },
    );
    expect(result).toEqual({ ok: true, items: [], nextCursor: "opaque-2" });
    expect(requests[0]?.method).toBe(escodeCircuitMethods.compareFirmware);
    expect(requests[0]?.params.workspaceKey).toBe("C:/work");
    expect(requests[0]?.params.args).toEqual({
      irId: "ir:1",
      files: ["firmware/board.dts"],
      cursor: "opaque-1",
    });
  });
});

describe("circuit 注册门控与批准", () => {
  const pluginOutcome = (enabled: boolean) => ({
    plugins: [{ id: OFFICIAL_CIRCUIT_PLUGIN_ID, enabled, rootPath: "/plugins/circuit" }],
  });

  it("Host 能力标记只认 ESCODE_HOST_CIRCUIT=1", () => {
    expect(isHostCircuitAvailable({ [ESCODE_HOST_CIRCUIT_ENV]: "1" })).toBe(true);
    expect(isHostCircuitAvailable({ [ESCODE_HOST_CIRCUIT_ENV]: "0" })).toBe(false);
    expect(isHostCircuitAvailable({})).toBe(false);
  });

  it("仅在拿到 broker 且插件启用时注册 circuit MCP server", () => {
    const resolve = (brokerAvailable: boolean, enabled: boolean) =>
      resolveBuiltInCircuitMcpServers({
        brokerAvailable,
        pluginOutcome: pluginOutcome(enabled) as never,
        workingDirectory: "/work",
      });
    expect(resolve(true, true).circuit).toMatchObject({
      type: "stdio",
      isolation: "workspace",
      // circuit server 拒绝旧版协商，注册必须固定现代协议版本（与 serial 一致）。
      protocolVersion: "2026-07-28",
    });
    expect(Object.keys(resolve(false, true))).toHaveLength(0);
    expect(Object.keys(resolve(true, false))).toHaveLength(0);
  });

  it("免批准名单与规格 §5 一致：只读分析工具在内，circuit_export 不在名单里", () => {
    const allowed = resolveOfficialPluginDefaultAllowedTools({
      enabledPluginIds: new Set([OFFICIAL_CIRCUIT_PLUGIN_ID]),
      registeredMcpServerNames: new Set(["circuit"]),
    }).sort();
    expect(allowed).toEqual([
      "mcp__circuit__circuit_check",
      "mcp__circuit__circuit_compare_firmware",
      "mcp__circuit__circuit_correct",
      "mcp__circuit__circuit_get",
      "mcp__circuit__circuit_parse",
      "mcp__circuit__circuit_query",
      "mcp__circuit__circuit_submit_recognition",
    ]);
    expect(allowed).not.toContain("mcp__circuit__circuit_export");
  });
});
