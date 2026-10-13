import { describe, expect, it } from "vitest";
import {
  CIRCUIT_RESPONSE_MAX_BYTES,
  CIRCUIT_TOOL_NAMES,
  circuitCorrectRequestSchema,
  circuitErrorCodeSchema,
  circuitQueryRequestSchema,
  circuitQueryResponseSchema,
  circuitReferenceSchema,
  circuitToolFailureSchema,
  circuitToolRequestSchemas,
  circuitToolResponseSchemas,
} from "../src/circuit/index.js";

const contentHash = "0123456789abcdef".repeat(4);

const summary = {
  irId: "ir-1",
  revision: 4,
  source: { kind: "kicad_sch" as const, path: "hardware/board.kicad_sch", contentHash },
  stale: false,
  componentCount: 2,
  netCount: 2,
  blockCount: 1,
  lowConfidenceCount: 0,
  diagnostics: { total: 0, error: 0, warning: 0, info: 0 },
};

const failure = { ok: false as const, error: { code: "stale_revision" as const, message: "版本落后" } };

describe("Circuit 工具契约信封", () => {
  it("固定工具名、错误码与响应上限", () => {
    expect(Object.values(CIRCUIT_TOOL_NAMES)).toEqual([
      "circuit_parse",
      "circuit_get",
      "circuit_query",
      "circuit_correct",
      "circuit_compare_firmware",
      "circuit_export",
    ]);
    expect(CIRCUIT_RESPONSE_MAX_BYTES).toBe(64 * 1024);
    for (const code of [
      "unavailable",
      "not_found",
      "unsupported_format",
      "invalid_ir",
      "stale_revision",
      "too_large",
      "io_error",
    ])
      expect(circuitErrorCodeSchema.safeParse(code).success).toBe(true);
    expect(circuitErrorCodeSchema.safeParse("timeout").success).toBe(false);
    expect(circuitToolFailureSchema.safeParse(failure).success).toBe(true);
    expect(
      circuitToolFailureSchema.safeParse({ ok: false, error: { code: "timeout", message: "x" } })
        .success,
    ).toBe(false);
    expect(circuitToolFailureSchema.safeParse({ ok: false, error: { code: "io_error" } }).success).toBe(false);
  });

  it("每个工具都接受失败响应，且成功响应拒绝多余字段", () => {
    for (const schema of Object.values(circuitToolResponseSchemas))
      expect(schema.safeParse(failure).success).toBe(true);

    expect(
      circuitToolResponseSchemas.parse.safeParse({ ok: true, summary, extra: 1 }).success,
    ).toBe(false);
    expect(circuitToolResponseSchemas.parse.safeParse({ ok: true, summary }).success).toBe(true);
  });
});

describe("Circuit 工具请求校验", () => {
  it("circuit_parse 只接受 workspace 相对路径，baseRevision 可选", () => {
    expect(circuitToolRequestSchemas.parse.safeParse({ path: "hw/board.kicad_sch" }).success).toBe(true);
    expect(
      circuitToolRequestSchemas.parse.safeParse({ path: "hw/board.kicad_sch", baseRevision: 3 }).success,
    ).toBe(true);
    expect(
      circuitToolRequestSchemas.parse.safeParse({ path: "/abs/board.kicad_sch" }).success,
    ).toBe(false);
    expect(
      circuitToolRequestSchemas.parse.safeParse({ path: "hw/board.kicad_sch", baseRevision: -1 }).success,
    ).toBe(false);
    expect(circuitToolRequestSchemas.parse.safeParse({}).success).toBe(false);
  });

  it("circuit_query 的四种模式：省略过滤 = 列出全部，id 与名字最多给一个", () => {
    const irId = "ir-1";
    expect(circuitQueryRequestSchema.safeParse({ irId, by: "ref", ref: "U1" }).success).toBe(true);
    // 面板的 BOM / 网络表：不带过滤条件列出该类全部元素。
    expect(circuitQueryRequestSchema.safeParse({ irId, by: "ref" }).success).toBe(true);
    expect(circuitQueryRequestSchema.safeParse({ irId, by: "net", netId: "net-1" }).success).toBe(true);
    expect(circuitQueryRequestSchema.safeParse({ irId, by: "net", netName: "3V3" }).success).toBe(true);
    expect(circuitQueryRequestSchema.safeParse({ irId, by: "net" }).success).toBe(true);
    expect(
      circuitQueryRequestSchema.safeParse({ irId, by: "net", netId: "net-1", netName: "3V3" }).success,
    ).toBe(false);
    expect(circuitQueryRequestSchema.safeParse({ irId, by: "block", blockName: "MCU" }).success).toBe(true);
    expect(circuitQueryRequestSchema.safeParse({ irId, by: "block" }).success).toBe(true);
    expect(
      circuitQueryRequestSchema.safeParse({ irId, by: "block", blockId: "b-1", blockName: "MCU" })
        .success,
    ).toBe(false);
    expect(circuitQueryRequestSchema.safeParse({ irId, by: "trace", componentId: "uuid-u1" }).success).toBe(true);
    expect(circuitQueryRequestSchema.safeParse({ irId, by: "trace", componentId: "uuid-u1", pin: "1" }).success).toBe(true);
    expect(circuitQueryRequestSchema.safeParse({ irId, by: "trace" }).success).toBe(false);
    expect(circuitQueryRequestSchema.safeParse({ irId, by: "pin", pin: "1" }).success).toBe(false);
    expect(
      circuitQueryRequestSchema.safeParse({ irId, by: "ref", ref: "U1", cursor: "c1" }).success,
    ).toBe(true);
  });

  it("circuit_query 响应按模式校验 items 类型与 nextCursor", () => {
    const component = {
      id: "uuid-u1",
      ref: "U1",
      pins: [],
      origin: { by: "parser", confidence: 1 },
    };
    const net = {
      id: "net-1",
      name: "3V3",
      pinRefs: [],
      origin: { by: "parser", confidence: 1 },
    };
    expect(
      circuitQueryResponseSchema.safeParse({ ok: true, by: "ref", items: [component] }).success,
    ).toBe(true);
    expect(
      circuitQueryResponseSchema.safeParse({
        ok: true,
        by: "ref",
        items: [component],
        nextCursor: "cursor-2",
      }).success,
    ).toBe(true);
    expect(circuitQueryResponseSchema.safeParse({ ok: true, by: "ref", items: [net] }).success).toBe(false);
    expect(circuitQueryResponseSchema.safeParse({ ok: true, by: "net", items: [net] }).success).toBe(true);
    expect(
      circuitQueryResponseSchema.safeParse({
        ok: true,
        by: "trace",
        items: [
          { componentId: "uuid-u1", pin: "1", netIds: ["net-1"], nets: [net], components: [component] },
        ],
      }).success,
    ).toBe(true);
    expect(circuitQueryResponseSchema.safeParse(failure).success).toBe(true);
  });

  it("circuit_correct 校验 ops 与 baseRevision", () => {
    const base = { irId: "ir-1", baseRevision: 4 };
    expect(
      circuitCorrectRequestSchema.safeParse({
        ...base,
        ops: [{ op: "update-component", componentId: "uuid-u1", ref: "U2" }],
      }).success,
    ).toBe(true);
    expect(
      circuitCorrectRequestSchema.safeParse({
        ...base,
        ops: [{ op: "update-component", componentId: "uuid-u1" }],
      }).success,
    ).toBe(false);
    expect(
      circuitCorrectRequestSchema.safeParse({
        ...base,
        ops: [{ op: "connect", componentId: "uuid-u1", pin: "1", netName: "3V3" }],
      }).success,
    ).toBe(true);
    expect(
      circuitCorrectRequestSchema.safeParse({
        ...base,
        ops: [{ op: "connect", componentId: "uuid-u1", pin: "1" }],
      }).success,
    ).toBe(false);
    expect(
      circuitCorrectRequestSchema.safeParse({
        ...base,
        ops: [{ op: "connect", componentId: "uuid-u1", pin: "1", netId: "net-1", netName: "3V3" }],
      }).success,
    ).toBe(false);
    expect(
      circuitCorrectRequestSchema.safeParse({
        ...base,
        ops: [{ op: "disconnect", componentId: "uuid-u1", pin: "1" }],
      }).success,
    ).toBe(true);
    expect(
      circuitCorrectRequestSchema.safeParse({
        ...base,
        ops: [{ op: "add-block", name: "电源", componentIds: ["uuid-u1"] }],
      }).success,
    ).toBe(true);
    expect(
      circuitCorrectRequestSchema.safeParse({
        ...base,
        ops: [{ op: "add-block", name: "电源", componentIds: [] }],
      }).success,
    ).toBe(false);
    expect(
      circuitCorrectRequestSchema.safeParse({
        ...base,
        ops: [{ op: "update-block", blockId: "block-1" }],
      }).success,
    ).toBe(false);
    expect(
      circuitCorrectRequestSchema.safeParse({
        ...base,
        ops: [{ op: "remove-block", blockId: "block-1" }],
      }).success,
    ).toBe(true);
    expect(
      circuitCorrectRequestSchema.safeParse({ ...base, ops: [{ op: "add-component", ref: "U9" }] })
        .success,
    ).toBe(false);
    expect(circuitCorrectRequestSchema.safeParse({ irId: "ir-1", ops: [] }).success).toBe(false);
    expect(
      circuitCorrectRequestSchema.safeParse({
        irId: "ir-1",
        ops: [{ op: "remove-block", blockId: "block-1" }],
      }).success,
    ).toBe(false);
  });
});

describe("Circuit 分析工具契约", () => {
  it("circuit_compare_firmware 校验固件文件与差异类型", () => {
    expect(
      circuitToolRequestSchemas.compareFirmware.safeParse({
        irId: "ir-1",
        files: ["firmware/app.overlay"],
      }).success,
    ).toBe(true);
    // §5 的分页对分析工具成立：游标是不透明字符串，可带可不带。
    expect(
      circuitToolRequestSchemas.compareFirmware.safeParse({
        irId: "ir-1",
        files: ["firmware/app.overlay"],
        cursor: "opaque-cursor",
      }).success,
    ).toBe(true);
    expect(
      circuitToolRequestSchemas.compareFirmware.safeParse({
        irId: "ir-1",
        files: ["firmware/app.overlay"],
        cursor: "",
      }).success,
    ).toBe(false);
    expect(
      circuitToolRequestSchemas.compareFirmware.safeParse({ irId: "ir-1", files: [] }).success,
    ).toBe(false);
    expect(
      circuitToolRequestSchemas.compareFirmware.safeParse({
        irId: "ir-1",
        files: ["/abs/firmware/app.overlay"],
      }).success,
    ).toBe(false);

    const response = {
      ok: true,
      items: [
        {
          kind: "pin-mismatch",
          pin: "PA5",
          netName: "LED",
          componentId: "uuid-u1",
          firmwareSymbol: "led0",
          file: "firmware/app.overlay",
          line: 12,
        },
      ],
    };
    expect(circuitToolResponseSchemas.compareFirmware.safeParse(response).success).toBe(true);
    expect(
      circuitToolResponseSchemas.compareFirmware.safeParse({
        ...response,
        items: [{ ...response.items[0], kind: "unknown-kind" }],
      }).success,
    ).toBe(false);
    expect(
      circuitToolResponseSchemas.compareFirmware.safeParse({
        ...response,
        items: [{ ...response.items[0], file: "/abs/firmware/app.overlay" }],
      }).success,
    ).toBe(false);
  });

  it("circuit_export 请求与响应同样是 workspace 相对路径", () => {
    expect(
      circuitToolRequestSchemas.export.safeParse({ irId: "ir-1", path: "out/board.circuit.json" }).success,
    ).toBe(true);
    expect(
      circuitToolRequestSchemas.export.safeParse({ irId: "ir-1", path: "../board.circuit.json" }).success,
    ).toBe(false);
    expect(
      circuitToolResponseSchemas.export.safeParse({ ok: true, path: "out/board.circuit.json" }).success,
    ).toBe(true);
  });

  it("::circuit 引用载荷接受可选标题与计数，拒绝未知字段", () => {
    expect(circuitReferenceSchema.safeParse({ irId: "ir-1" }).success).toBe(true);
    expect(
      circuitReferenceSchema.safeParse({ irId: "ir-1", title: "电源板", componentCount: 12, netCount: 30 })
        .success,
    ).toBe(true);
    expect(circuitReferenceSchema.safeParse({ irId: "ir-1", componentCount: -1 }).success).toBe(false);
    expect(circuitReferenceSchema.safeParse({ irId: "ir-1", path: "x" }).success).toBe(false);
  });
});
