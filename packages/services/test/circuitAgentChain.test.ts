/**
 * Agent 电路工具的整链路测试（docs/circuit/README.md §5、§10.2）：不经过模型，
 * 真实 MCP client → circuit MCP server 进程（circuit-plugin 构建产物）→ TS circuit broker
 * → 协议端口 →（模拟 JSON-RPC 往返）→ Host 路由与 CircuitAgentBridge → CircuitService。
 * 运行前需构建 circuit-plugin：pnpm --filter @escode/circuit-plugin build
 */
import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { Client } from "@modelcontextprotocol/client";
import { StdioClientTransport } from "@modelcontextprotocol/client/stdio";
import {
  CIRCUIT_BROKER_SOCKET_ENV,
  CIRCUIT_BROKER_TOKEN_ENV,
  circuitToolResponseSchemas,
} from "@escode/shared/circuit";
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
const FIRMWARE_DIR = fileURLToPath(new URL("./fixtures/circuit/firmware/", import.meta.url));
const SESSION = "session-chain";
const WORKSPACE = "C:/work/board";
const SOURCE_PATH = "MPU6050.kicad_sch";
/** 固件比对用的原理图（MCU 引脚名与 fixtures/circuit/firmware 下的固件声明对应）。 */
const FIRMWARE_SOURCE_PATH = "firmware/schematic.kicad_sch";
const RULES_CONFIG_PATH = ".escode/circuit-rules.json";
/** 识别链路的源文件：矢量 PDF 与截图各一份（§5 circuit_submit_recognition）。 */
const RECOGNITION_PDF_PATH = "recognition/board.pdf";
const RECOGNITION_IMAGE_PATH = "recognition/board.png";
const RECOGNITION_DRAFT = {
  components: [
    {
      id: "draft-u1",
      ref: "U1",
      value: "MPU-6050",
      pins: [{ number: "1", name: "VDD", netId: "draft-net-3v3" }],
      origin: { by: "recognition", region: { page: 1, x: 50, y: 80, w: 40, h: 30 } },
    },
  ],
  nets: [
    {
      id: "draft-net-3v3",
      name: "3V3",
      pinRefs: [{ componentId: "draft-u1", pin: "1" }],
      origin: { by: "recognition" },
    },
  ],
};
const META = {
  "com.escode/request-context": { session_id: SESSION, turn_id: "turn-1", runtime_scope: "main" },
};

let service: CircuitService;
let broker: CircuitBroker;
let client: Client;
let schematic: string;
/** beforeAll 里直接解析一次（重新解析已有 IR 需要 baseRevision，分析测试只读它）。 */
let firmwareIrId: string;
/** 导出快照落点（真实服务只写 workspace 内的派生产物，这里用内存端口捕获）。 */
const snapshots = new Map<string, string>();

function createPorts(
  files: Map<string, string>,
  binaries: Map<string, Uint8Array>,
): CircuitServicePorts {
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
      async readBytes({ relativePath }) {
        const bytes = binaries.get(relativePath);
        if (bytes === undefined) throw new Error(`not_found: ${relativePath}`);
        return bytes;
      },
      async exists({ relativePath }) {
        return files.has(relativePath) || binaries.has(relativePath);
      },
      hashContent: (text) => createHash("sha256").update(text).digest("hex"),
      hashBytes: (bytes) => createHash("sha256").update(bytes).digest("hex"),
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
    vector: {
      // 整链路测试不引入 pdfjs：识别 PDF 时给出固定的矢量证据（真实适配器见 circuit/pdfVector.test.ts）。
      async extract() {
        return { pageCount: 1, texts: [{ page: 1, text: "U1", x: 60, y: 90 }], segments: [] };
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
  const files = new Map([
    [SOURCE_PATH, schematic],
    [FIRMWARE_SOURCE_PATH, await readFile(join(FIRMWARE_DIR, "schematic.kicad_sch"), "utf-8")],
    ["firmware/board.dts", await readFile(join(FIRMWARE_DIR, "board.dts"), "utf-8")],
    ["firmware/board.ioc", await readFile(join(FIRMWARE_DIR, "board.ioc"), "utf-8")],
    ["firmware/board_pins.h", await readFile(join(FIRMWARE_DIR, "board_pins.h"), "utf-8")],
    // §6.1 的规则配置：check 读取它并把 severity 覆写反映到条目（配置由端口提供，不落盘）。
    [RULES_CONFIG_PATH, JSON.stringify({ rules: { "unconnected-pin": { severity: "info" } } })],
  ]);
  const binaries = new Map([
    // 识别链路的源文件（真实内容无关紧要：矢量证据由端口给出，读字节走真实端口实现）。
    [
      RECOGNITION_PDF_PATH,
      new Uint8Array(Buffer.from("%PDF-1.4 recognition chain test", "latin1")),
    ],
    [RECOGNITION_IMAGE_PATH, new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])],
  ]);
  service = createCircuitService({ ports: createPorts(files, binaries) });
  // 分析工具只读：这里先把固件比对用的原理图解析好（链路上重新解析需要 baseRevision）。
  firmwareIrId = (
    await service.parse({ workspacePath: WORKSPACE, path: FIRMWARE_SOURCE_PATH })
  ).summary.irId;
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

it("工具清单经真实 MCP server 暴露已注册的八个工具", async () => {
  const { tools } = await client.listTools();
  expect(tools.map((tool) => tool.name).sort()).toEqual([
    "circuit_check",
    "circuit_compare_firmware",
    "circuit_correct",
    "circuit_export",
    "circuit_get",
    "circuit_parse",
    "circuit_query",
    "circuit_submit_recognition",
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

  it("circuit_check 走通整条链路：读 workspace 规则配置，条目按配置覆写级别", async () => {
    const checked = await call("circuit_check", { irId: firmwareIrId });
    expect(checked.isError).toBeUndefined();
    const envelope = json(checked);
    expect(envelope.config).toEqual({ status: "applied", path: RULES_CONFIG_PATH });
    expect(Array.isArray(envelope.items)).toBe(true);
    for (const item of envelope.items as Array<{ ruleId: string; severity: string }>)
      if (item.ruleId === "unconnected-pin") expect(item.severity).toBe("info");
    expect(circuitToolResponseSchemas.check.safeParse(envelope).success).toBe(true);
    // 游标与查询绑定：拿别的查询的游标翻页报 invalid_ir（服务端定义，链路只透传）。
    const wrongCursor = await call("circuit_check", { irId: firmwareIrId, cursor: "bogus" });
    expect(wrongCursor.isError).toBe(true);
    expect(json(wrongCursor)).toMatchObject({ ok: false, error: { code: "invalid_ir" } });
  });

  it("circuit_compare_firmware 比对固件与原理图，格式错误同样走失败信封", async () => {
    const compared = await call("circuit_compare_firmware", {
      irId: firmwareIrId,
      files: ["firmware/board.dts", "firmware/board.ioc", "firmware/board_pins.h"],
    });
    expect(compared.isError).toBeUndefined();
    const envelope = json(compared);
    expect(envelope.ok).toBe(true);
    expect((envelope.items as unknown[])[0]).toMatchObject({
      kind: "pin-mismatch",
      pin: "PA9",
      netName: "LED2",
      firmwareSymbol: "LED1",
      file: "firmware/board.dts",
      line: 21,
    });
    expect(circuitToolResponseSchemas.compareFirmware.safeParse(envelope).success).toBe(true);

    // 扩展名先判后读：不支持的文件直接报 unsupported_format，不读盘。
    const unsupported = await call("circuit_compare_firmware", {
      irId: firmwareIrId,
      files: ["firmware/notes.txt"],
    });
    expect(unsupported.isError).toBe(true);
    expect(json(unsupported)).toMatchObject({ ok: false, error: { code: "unsupported_format" } });
  });

  it("circuit_submit_recognition 走通整条链路：草稿 → 服务端 id 与置信度 → 摘要", async () => {
    const pdf = json(
      await call("circuit_submit_recognition", {
        path: RECOGNITION_PDF_PATH,
        draft: RECOGNITION_DRAFT,
      }),
    );
    expect(pdf.ok).toBe(true);
    const summary = pdf.summary as {
      irId: string;
      revision: number;
      source: { kind: string; contentHash: string };
      componentCount: number;
      lowConfidenceCount: number;
      diagnostics: { total: number };
    };
    expect(summary.revision).toBe(1);
    expect(summary.source.kind).toBe("pdf_vector");
    expect(summary.componentCount).toBe(1);
    // U1 在矢量 PDF 的文本证据里出现 → 按确定性解析处理；3V3 只有一个引脚 → 0.8，都还不算低置信度。
    expect(summary.lowConfidenceCount).toBe(0);
    expect(summary.diagnostics.total).toBeGreaterThan(0);

    // 元素 id 由服务端生成，草稿 id 不再出现在 IR 里。
    const byRef = json(await call("circuit_query", { irId: summary.irId, by: "ref", ref: "U1" }));
    const component = (
      byRef.items as Array<{ id: string; origin: { by: string; confidence: number } }>
    )[0]!;
    expect(component.id).toBe("recognition:U1");
    expect(component.origin).toEqual({
      by: "parser",
      confidence: 1,
      region: { page: 1, x: 50, y: 80, w: 40, h: 30 },
    });

    // 同一份内容重新提交必须带 baseRevision（与 parse 前置条件一致）。
    const noRevision = await call("circuit_submit_recognition", {
      path: RECOGNITION_PDF_PATH,
      draft: RECOGNITION_DRAFT,
    });
    expect(noRevision.isError).toBe(true);
    expect(json(noRevision)).toMatchObject({ ok: false, error: { code: "invalid_ir" } });

    const staleRevision = await call("circuit_submit_recognition", {
      path: RECOGNITION_PDF_PATH,
      draft: RECOGNITION_DRAFT,
      baseRevision: 7,
    });
    expect(json(staleRevision)).toMatchObject({ ok: false, error: { code: "stale_revision" } });

    // 截图走同一工具，没有矢量证据：所有元素按扣分规则给分，永不等于 1。
    const image = json(
      await call("circuit_submit_recognition", {
        path: RECOGNITION_IMAGE_PATH,
        draft: RECOGNITION_DRAFT,
      }),
    );
    const imageSummary = image.summary as { source: { kind: string }; irId: string };
    expect(imageSummary.source.kind).toBe("image");
    const imageRef = json(
      await call("circuit_query", { irId: imageSummary.irId, by: "ref", ref: "U1" }),
    );
    const imageComponent = (
      imageRef.items as Array<{ origin: { by: string; confidence: number } }>
    )[0]!;
    expect(imageComponent.origin).toMatchObject({ by: "recognition", confidence: 0.9 });

    // 非图片/PDF 的来源由服务端判格式：确定性解析走 circuit_parse。
    const unsupported = await call("circuit_submit_recognition", {
      path: SOURCE_PATH,
      draft: RECOGNITION_DRAFT,
    });
    expect(unsupported.isError).toBe(true);
    expect(json(unsupported)).toMatchObject({ ok: false, error: { code: "unsupported_format" } });
  });

  it("没有会话上下文时返回 unavailable", async () => {
    const noContext = (await client.callTool({
      name: "circuit_get",
      arguments: { irId: "ir:1" },
    })) as ToolResult;
    expect(noContext.content[0]!.text).toMatch(/^\[unavailable\]/u);
  });
});
