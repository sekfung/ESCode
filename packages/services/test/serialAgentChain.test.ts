/**
 * Agent 串口工具的整链路测试（docs/specs/serial-agent-tools.md「验收」）：不经过模型，
 * 真实 MCP client → serial MCP server 进程（serial-plugin 构建产物）→ TS serial broker → 协议端口
 * →（模拟 JSON-RPC 往返）→ Host 路由与 SerialAgentBridge → SerialService（binding-mock 回环虚拟串口）。
 * 运行前需构建 serial-plugin：pnpm --filter @escode/serial-plugin build
 */
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { after, before, test } from "node:test";
import { Client } from "@modelcontextprotocol/client";
import { StdioClientTransport } from "@modelcontextprotocol/client/stdio";
import { SERIAL_BROKER_SOCKET_ENV, SERIAL_BROKER_TOKEN_ENV } from "@escode/shared/serial";
import {
  createSerialBroker,
  type SerialBroker,
} from "../../../apps/escode-cli/packages/bootstrap/src/app/serial-broker.js";
import { createProtocolSerialControlPort } from "../../../apps/escode-cli/packages/bootstrap/src/escode-protocol/serial-control-broker.js";
import { ProtocolRequestError } from "../../../apps/escode-cli/packages/bootstrap/src/escode-protocol/server-types.js";
import { createSerialAgentBridge } from "../src/serial/serialAgentBridge.js";
import { routeSerialAgentRequest } from "../src/serial/serialAgentRequestRouter.js";
import {
  createSerialService,
  loadDefaultSerialBinding,
  type SerialService,
} from "../src/serial/serialService.js";

const SERVER_SCRIPT = resolve(
  import.meta.dirname,
  "../../../apps/escode-cli/packages/serial-plugin/dist/mcp/server.js",
);
const SESSION = "session-chain";
const META = {
  "com.escode/request-context": { session_id: SESSION, turn_id: "turn-1", runtime_scope: "main" },
};
const logger = { debug() {}, info() {}, warn() {}, error() {}, child: () => logger } as never;

let service: SerialService;
let broker: SerialBroker;
let client: Client;

before(async () => {
  assert.ok(
    existsSync(SERVER_SCRIPT),
    `missing ${SERVER_SCRIPT}; build @escode/serial-plugin first`,
  );
  process.env.ESCODE_SERIAL_MOCK_PORTS = "COM_CHAIN1,COM_CHAIN2";
  service = createSerialService({ loadBinding: loadDefaultSerialBinding, coalesceWindowMs: 0 });
  const bridge = createSerialAgentBridge({
    getSerialService: () => service,
    getRememberedAutoReconnect: async () => undefined,
  });
  // 协议上下文：requestClient 模拟 Agent→Host 的 JSON-RPC 往返，Host 侧走真实路由。
  const context = {
    sessions: new Map([[SESSION, { workspace: { workspacePath: "C:/work" } }]]),
    deps: {},
    requestClient: (method: string, params: unknown, schema: { parse(v: unknown): unknown }) =>
      new Promise((resolveRequest, rejectRequest) => {
        const handled = routeSerialAgentRequest({
          request: { id: 1, method, params },
          bridge,
          responder: {
            respond: async (_id, result) => resolveRequest(schema.parse(result)),
            respondError: async (_id, error) =>
              rejectRequest(new ProtocolRequestError(error.code, error.message, error.data)),
          },
        });
        if (!handled) rejectRequest(new Error(`unrouted ${method}`));
      }),
  };
  broker = createSerialBroker({ port: createProtocolSerialControlPort(context as never), logger });
  await broker.ready;
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [SERVER_SCRIPT],
    env: {
      ...(process.env as Record<string, string>),
      [SERIAL_BROKER_SOCKET_ENV]: broker.socketPath,
      [SERIAL_BROKER_TOKEN_ENV]: broker.token,
    },
  });
  // 与 Agent 对内置 serial server 的注册一致（protocolVersion 2026-07-28 → pin 协商）。
  client = new Client(
    { name: "serial-chain-test", version: "0.0.0" },
    { versionNegotiation: { mode: { pin: "2026-07-28" } } },
  );
  await client.connect(transport);
});

after(async () => {
  await client?.close().catch(() => undefined);
  await broker?.close();
  await service?.disposeAllAndWait();
  delete process.env.ESCODE_SERIAL_MOCK_PORTS;
});

async function call(name: string, args: Record<string, unknown> = {}) {
  const result = (await client.callTool({ name, arguments: args, _meta: META })) as {
    content: Array<{ type: string; text: string }>;
    isError?: boolean;
  };
  return result;
}
const json = (result: { content: Array<{ text: string }> }, index = 0) =>
  JSON.parse(result.content[index]!.text) as Record<string, unknown>;

test("工具清单经真实 MCP server 暴露全部串口工具", async () => {
  const { tools } = await client.listTools();
  assert.deepEqual(tools.map((tool) => tool.name).sort(), [
    "serial_close",
    "serial_list",
    "serial_open",
    "serial_read",
    "serial_set_signals",
    "serial_wait_for",
    "serial_write",
  ]);
});

test("list → open → write → wait_for（回环）→ read → close 走通整条链路", async () => {
  const listed = json(await call("serial_list"));
  assert.deepEqual(
    (listed.ports as Array<{ path: string }>).map((port) => port.path),
    ["COM_CHAIN1", "COM_CHAIN2"],
  );

  const opened = json(await call("serial_open", { path: "COM_CHAIN1", baudRate: 115200 }));
  assert.equal(opened.reused, false);

  const cursor = json(await call("serial_read"), 1).lastSeq as number;
  const waiting = call("serial_wait_for", { pattern: "PING", sinceSeq: cursor, timeoutMs: 5000 });
  const written = json(await call("serial_write", { data: "PING", lineEnding: "crlf" }));
  assert.equal(written.bytes, 6);
  const matched = json(await waiting);
  assert.equal(matched.matched, true);
  assert.equal(matched.match, "PING");

  const read = await call("serial_read", { sinceSeq: cursor, direction: "both" });
  assert.equal(read.content[0]!.text, "TX PING\r\nRX PING\r\n");

  // Agent 写入在 Host 收发记录里带会话来源，供面板标注 [Agent·…]。
  const tx = (await service.getSnapshot({ path: "COM_CHAIN1" })).chunks.find(
    (chunk) => chunk.direction === "tx",
  );
  assert.equal(tx?.source, "agent");
  assert.equal(tx?.sessionId, SESSION);

  const closed = json(await call("serial_close"));
  assert.equal((closed.status as { state: string }).state, "closed");
});

test("不抢占：用户已打开的串口参数不同时 open 返回 busy", async () => {
  await service.open({
    path: "COM_CHAIN1",
    config: {
      baudRate: 9600,
      dataBits: 8,
      parity: "none",
      stopBits: 1,
      rtscts: false,
      autoReconnect: true,
    },
  });
  const result = await call("serial_open", { path: "COM_CHAIN1", baudRate: 115200 });
  assert.equal(result.isError, true);
  assert.match(result.content[0]!.text, /^\[busy\]/);
  await service.close({ path: "COM_CHAIN1" });
});

test("业务错误码与参数错误都以 [code] 形式返回给模型", async () => {
  const notOpen = await call("serial_write", { data: "AT" });
  assert.equal(notOpen.isError, true);
  assert.match(notOpen.content[0]!.text, /^\[notOpen\]/);
  const invalid = await call("serial_read", { maxBytes: 0 });
  assert.match(invalid.content[0]!.text, /^\[invalidInput\]/);
  const noContext = (await client.callTool({ name: "serial_list", arguments: {} })) as {
    content: Array<{ text: string }>;
  };
  assert.match(noContext.content[0]!.text, /^\[unavailable\]/);
});

test("多串口：两个串口同时打开时省略 path 被拒绝，带 path 各自收发", async () => {
  await call("serial_open", { path: "COM_CHAIN1", baudRate: 115200 });
  await call("serial_open", { path: "COM_CHAIN2", baudRate: 115200 });
  const ambiguous = await call("serial_write", { data: "X" });
  assert.equal(ambiguous.isError, true);
  assert.match(ambiguous.content[0]!.text, /^\[invalidInput\].*COM_CHAIN1.*COM_CHAIN2/);
  const written = json(await call("serial_write", { path: "COM_CHAIN2", data: "Y" }));
  assert.equal(written.bytes, 1);
  const second = (await service.getSnapshot({ path: "COM_CHAIN2" })).chunks.map((c) =>
    Buffer.from(c.bytes).toString(),
  );
  assert.ok(second.includes("Y"));
  assert.deepEqual(
    (await service.getSnapshot({ path: "COM_CHAIN1" })).chunks.filter(
      (c) => c.direction === "tx" && Buffer.from(c.bytes).toString() === "Y",
    ),
    [],
  );
  const listed = json(await call("serial_list"));
  assert.equal((listed.sessions as unknown[]).length, 2);
  await call("serial_close", { path: "COM_CHAIN1" });
  await call("serial_close", { path: "COM_CHAIN2" });
});

test("serial_set_signals 经整条链路设置 DTR/RTS 与复位脉冲", async () => {
  await call("serial_open", { path: "COM_CHAIN1", baudRate: 115200 });
  const set = json(await call("serial_set_signals", { dtr: false }));
  assert.deepEqual(set.signals, { dtr: false, rts: true });
  const pulsed = json(await call("serial_set_signals", { pulse: "esp32" }));
  assert.deepEqual(pulsed.signals, { dtr: false, rts: true });
  const conflicting = await call("serial_set_signals", { pulse: "esp32", dtr: true });
  assert.match(conflicting.content[0]!.text, /^\[invalidInput\]/);
  await call("serial_close");
});
