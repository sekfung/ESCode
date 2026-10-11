/**
 * Agent 电路工具的整链路测试（docs/circuit/README.md §5、§10.2）：不经过模型，
 * 真实 MCP client → circuit MCP server 进程（circuit-plugin 构建产物）→ TS circuit broker
 * → 协议端口 →（模拟 JSON-RPC 往返）→ Host 路由与 CircuitAgentBridge → CircuitService。
 * 运行前需构建 circuit-plugin：pnpm --filter @escode/circuit-plugin build
 */
import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { Client } from "@modelcontextprotocol/client";
import { StdioClientTransport } from "@modelcontextprotocol/client/stdio";
import { CIRCUIT_BROKER_SOCKET_ENV, CIRCUIT_BROKER_TOKEN_ENV } from "@escode/shared/circuit";
import {
  createCircuitBroker,
  type CircuitBroker,
} from "../../../apps/escode-cli/packages/bootstrap/src/app/circuit-broker.js";
import { createProtocolCircuitControlPort } from "../../../apps/escode-cli/packages/bootstrap/src/escode-protocol/circuit-control-broker.js";
import { ProtocolRequestError } from "../../../apps/escode-cli/packages/bootstrap/src/escode-protocol/server-types.js";
import { createCircuitService } from "../src/circuit/app/circuitService.js";
import { createCircuitAgentBridge } from "../src/circuit/app/circuitAgentBridge.js";
import { routeCircuitAgentRequest } from "../src/circuit/app/circuitAgentRequestRouter.js";
import type { StoredCircuit, CircuitServicePorts } from "../src/circuit/app/ports.js";
import type { CircuitService } from "../src/circuit/app/circuitService.js";

const SERVER_SCRIPT = resolve(
  import.meta.dirname,
  "../../../apps/escode-cli/packages/circuit-plugin/dist/mcp/server.js",
);
const FIXTURE = fileURLToPath(
  new URL("./fixtures/circuit/mpu6050-imu/MPU6050.kicad_sch", import.meta.url),
);
const SESSION = "session-chain";
const WORKSPACE = "C:/work/board";
const SOURCE_PATH = "MPU6050.kicad_sch";
const META = {
  "com.escode/request-context": { session_id: SESSION, turn_id: "turn-1", runtime_scope: "main" },
};

let service: CircuitService;
let broker: CircuitBroker;
let client: Client;
let schematic: string;
/** 导出快照落点（真实服务只写 workspace 内的派生产物，这里用内存端口捕获）。 */
const snapshots = new Map<string, string>();

function createPorts(files: Map<string, string>): CircuitServicePorts {
  const records = new Map<string, StoredCircuit>();
  const key = (identityKey: string, sourcePath: string) => `${identityKey}\u0000${sourcePath}`;
  return {
    store: {
      async read({ identityKey, sourcePath }) {
        const record = records.get(key(identityKey, sourcePath));
        return record ? structuredClone(record) : undefined;
      },
      async findByIrId({ identityKey, irId }) {
        for (const [entryKey, record] of records)
          if (entryKey.startsWith(`${identityKey}\u0000`) && record.irId === irId)
            return structuredClone(record);
        return undefined;
      },
      async write({ identityKey, sourcePath, record }) {
        records.set(key(identityKey, sourcePath), structuredClone(record));
      },
    },
    source: {
      async readText({ relativePath }) {
        const text = files.get(relativePath);
        if (text === undefined) throw new Error(`not_found: ${relativePath}`);
        return text;
      },
      async exists({ relativePath }) {
        return files.has(relativePath);
      },
      hashContent: (text) => createHash("sha256").update(text).digest("hex"),
    },
    watcher: {
      async watch() {
        return { dispose: () => undefined };
      },
    },
    snapshot: {
      async write({ relativePath, content }) {
        snapshots.set(relativePath, content);
      },
    },
  };
}

beforeAll(async () => {
  expect(
    existsSync(SERVER_SCRIPT),
    `missing ${SERVER_SCRIPT}; build @escode/circuit-plugin first`,
  ).toBe(true);
  schematic = await readFile(FIXTURE, "utf-8");
  const files = new Map([[SOURCE_PATH, schematic]]);
  service = createCircuitService({ ports: createPorts(files) });
  const bridge = createCircuitAgentBridge({ getCircuitService: () => service });
  // 协议上下文：requestClient 模拟 Agent→Host 的 JSON-RPC 往返，Host 侧走真实路由。
  const context = {
    sessions: new Map([
      [
        SESSION,
        { workspace: { workspacePath: WORKSPACE, workspaceIdentity: "identity-1" } },
      ],
    ]),
    deps: {},
    requestClient: (method: string, params: unknown, schema: { parse(value: unknown): unknown }) =>
      new Promise((resolveRequest, rejectRequest) => {
        const handled = routeCircuitAgentRequest({
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
  broker = createCircuitBroker({
    port: createProtocolCircuitControlPort(context as never),
    logger: { warn() {}, error() {}, info() {}, debug() {}, child: () => undefined } as never,
  });
  await broker.ready;
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [SERVER_SCRIPT],
    env: {
      ...(process.env as Record<string, string>),
      [CIRCUIT_BROKER_SOCKET_ENV]: broker.socketPath,
      [CIRCUIT_BROKER_TOKEN_ENV]: broker.token,
    },
  });
  // 与 Agent 对内置 circuit server 的注册一致（protocolVersion 2026-07-28 → pin 协商）。
  client = new Client(
    { name: "circuit-chain-test", version: "0.0.0" },
    { versionNegotiation: { mode: { pin: "2026-07-28" } } },
  );
  await client.connect(transport);
});

afterAll(async () => {
  await client?.close().catch(() => undefined);
  await broker?.close();
  service?.dispose();
});

type ToolResult = { content: Array<{ type: string; text: string }>; isError?: boolean };

async function call(name: string, args: Record<string, unknown> = {}): Promise<ToolResult> {
  return (await client.callTool({ name, arguments: args, _meta: META })) as ToolResult;
}

const json = (result: ToolResult) => JSON.parse(result.content[0]!.text) as Record<string, unknown>;

it("工具清单经真实 MCP server 暴露 G4 的五个工具", async () => {
  const { tools } = await client.listTools();
  expect(tools.map((tool) => tool.name).sort()).toEqual([
    "circuit_correct",
    "circuit_export",
    "circuit_get",
    "circuit_parse",
    "circuit_query",
  ]);
});

describe("整链路", () => {
  it("parse → get → query（分页）→ correct 走通整条链路", async () => {
    const parsed = json(await call("circuit_parse", { path: SOURCE_PATH }));
    expect(parsed.ok).toBe(true);
    const summary = parsed.summary as { irId: string; revision: number; componentCount: number };
    expect(summary.revision).toBe(1);
    expect(summary.componentCount).toBeGreaterThan(0);

    const got = json(await call("circuit_get", { irId: summary.irId }));
    expect(got).toMatchObject({ ok: true, truncated: false });

    const byRef = json(await call("circuit_query", { irId: summary.irId, by: "ref", ref: "U1" }));
    expect(byRef).toMatchObject({ ok: true, by: "ref" });
    const component = (byRef.items as Array<{ id: string }>)[0]!;

    const traced = json(
      await call("circuit_query", { irId: summary.irId, by: "trace", componentId: component.id }),
    );
    expect((traced.items as unknown[]).length).toBeGreaterThan(0);

    const corrected = json(
      await call("circuit_correct", {
        irId: summary.irId,
        baseRevision: 1,
        ops: [{ op: "update-component", componentId: component.id, value: "chain-test" }],
      }),
    );
    expect(corrected).toMatchObject({ ok: true, summary: { revision: 2 } });
  });

  it("过期 baseRevision 与不存在的 irId 以规格错误码返回", async () => {
    const stale = await call("circuit_parse", { path: SOURCE_PATH, baseRevision: 1 });
    expect(stale.isError).toBe(true);
    expect(json(stale)).toMatchObject({ ok: false, error: { code: "stale_revision" } });

    const missing = await call("circuit_get", { irId: "ir:missing" });
    expect(missing.isError).toBe(true);
    expect(json(missing)).toMatchObject({ ok: false, error: { code: "not_found" } });
  });

  it("circuit_export 由 Host 写入派生产物快照，扩展名不对时报 unsupported_format", async () => {
    const parsed = json(await call("circuit_parse", { path: SOURCE_PATH, baseRevision: 2 }));
    const irId = (parsed.summary as { irId: string }).irId;

    const wrongExtension = await call("circuit_export", { irId, path: "out/board.json" });
    expect(wrongExtension.isError).toBe(true);
    expect(json(wrongExtension)).toMatchObject({
      ok: false,
      error: { code: "unsupported_format" },
    });

    const exported = json(await call("circuit_export", { irId, path: "out/board.circuit.json" }));
    expect(exported).toEqual({ ok: true, path: "out/board.circuit.json" });
    const snapshot = JSON.parse(snapshots.get("out/board.circuit.json")!) as {
      derived: boolean;
      irId: string;
      ir: { revision: number };
    };
    expect(snapshot.derived).toBe(true);
    expect(snapshot.irId).toBe(irId);
    expect(snapshot.ir.revision).toBe(3);
  });

  it("没有会话上下文时返回 unavailable", async () => {
    const noContext = (await client.callTool({
      name: "circuit_get",
      arguments: { irId: "ir:1" },
    })) as ToolResult;
    expect(noContext.content[0]!.text).toMatch(/^\[unavailable\]/u);
  });
});
