import { describe, expect, it } from "vitest";
import {
  CIRCUIT_IR_SCHEMA_VERSION,
  CIRCUIT_LOW_CONFIDENCE_THRESHOLD,
  circuitContentHashSchema,
  circuitIrSchema,
  circuitOriginSchema,
  circuitRecognitionDraftSchema,
  circuitRegionSchema,
  circuitWorkspacePathSchema,
  countCircuitDiagnostics,
  countLowConfidenceElements,
  isLowConfidence,
  toCircuitIrSummary,
  type CircuitIR,
} from "../src/circuit/index.js";

const contentHash = "0123456789abcdef".repeat(4);

function buildIr(overrides: Partial<CircuitIR> = {}): CircuitIR {
  return {
    schemaVersion: CIRCUIT_IR_SCHEMA_VERSION,
    irId: "ir-1",
    revision: 2,
    source: { kind: "kicad_sch", path: "hardware/board.kicad_sch", contentHash },
    stale: false,
    components: [
      {
        id: "uuid-u1",
        ref: "U1",
        value: "STM32F103C8T6",
        pins: [
          { number: "1", name: "VBAT", netId: "net-3v3" },
          { number: "2", name: "GND", netId: "net-gnd" },
        ],
        origin: { by: "parser", confidence: 1 },
      },
      {
        id: "uuid-c1",
        ref: "C1",
        value: "100nF",
        pins: [
          { number: "1", netId: "net-3v3" },
          { number: "2", netId: "net-gnd" },
        ],
        origin: { by: "recognition", confidence: 0.4 },
      },
    ],
    nets: [
      {
        id: "net-3v3",
        name: "3V3",
        pinRefs: [
          { componentId: "uuid-u1", pin: "1" },
          { componentId: "uuid-c1", pin: "1" },
        ],
        origin: { by: "parser", confidence: 1 },
      },
      {
        id: "net-gnd",
        name: "GND",
        pinRefs: [{ componentId: "uuid-u1", pin: "2" }],
        origin: { by: "parser", confidence: 1 },
      },
    ],
    blocks: [
      {
        id: "block-mcu",
        name: "MCU",
        componentIds: ["uuid-u1"],
        origin: { by: "correction", confidence: 1, author: { kind: "user", sessionId: "s-1" } },
      },
    ],
    diagnostics: [
      { code: "single-pin-net", severity: "warning", message: "网络 GND 只连了一个引脚", netIds: ["net-gnd"] },
      { code: "duplicate-ref", severity: "error", message: "位号 U1 重复", componentIds: ["uuid-u1"] },
    ],
    ...overrides,
  };
}

describe("Circuit IR v1 校验", () => {
  it("接受完整合法的 IR，并拒绝多余字段与缺失必填字段", () => {
    expect(circuitIrSchema.safeParse(buildIr()).success).toBe(true);

    const withExtra = { ...buildIr(), confidence: 0.5 };
    expect(circuitIrSchema.safeParse(withExtra).success).toBe(false);

    const { source: _source, ...missingSource } = buildIr();
    expect(circuitIrSchema.safeParse(missingSource).success).toBe(false);

    expect(circuitIrSchema.safeParse({ ...buildIr(), schemaVersion: 2 }).success).toBe(false);

    expect(
      circuitIrSchema.safeParse({
        ...buildIr(),
        diagnostics: [{ code: "dangling-net", severity: "fatal", message: "x" }],
      }).success,
    ).toBe(false);
    expect(
      circuitIrSchema.safeParse({
        ...buildIr(),
        diagnostics: [{ code: "unknown-code", severity: "info", message: "x" }],
      }).success,
    ).toBe(false);
  });

  it("只允许服务端写 confidence：parser=1、recognition<1、correction 必须带 author", () => {
    expect(
      circuitOriginSchema.safeParse({ by: "parser", confidence: 1 }).success,
    ).toBe(true);
    expect(
      circuitOriginSchema.safeParse({ by: "parser", confidence: 0.99 }).success,
    ).toBe(false);
    expect(
      circuitOriginSchema.safeParse({ by: "recognition", confidence: 0.99 }).success,
    ).toBe(true);
    expect(
      circuitOriginSchema.safeParse({ by: "recognition", confidence: 1 }).success,
    ).toBe(false);
    expect(
      circuitOriginSchema.safeParse({ by: "correction", confidence: 1 }).success,
    ).toBe(false);
    expect(
      circuitOriginSchema.safeParse({
        by: "correction",
        confidence: 1,
        author: { kind: "agent", sessionId: "s-9" },
      }).success,
    ).toBe(true);
    expect(circuitOriginSchema.safeParse({ by: "parser", confidence: 1.2 }).success).toBe(false);
    expect(circuitOriginSchema.safeParse({ by: "parser", confidence: -0.1 }).success).toBe(false);
  });

  it("校验源文件路径、内容哈希与识别区域", () => {
    for (const path of ["board.kicad_sch", "hardware/board.kicad_sch", "hardware\\sub\\board.kicad_pcb"])
      expect(circuitWorkspacePathSchema.safeParse(path).success).toBe(true);
    for (const path of [
      "/workspace/board.kicad_sch",
      "C:\\workspace\\board.kicad_sch",
      "\\\\server\\share\\board.kicad_sch",
      "../board.kicad_sch",
      "hardware/../../board.kicad_sch",
      "hardware//board.kicad_sch",
      "hardware/board.kicad_sch/",
      "",
      "line\nbreak.kicad_sch",
    ])
      expect(circuitWorkspacePathSchema.safeParse(path).success).toBe(false);

    expect(circuitContentHashSchema.safeParse(contentHash).success).toBe(true);
    for (const hash of [contentHash.toUpperCase(), contentHash.slice(1), `${contentHash}00`, "zz"])
      expect(circuitContentHashSchema.safeParse(hash).success).toBe(false);

    expect(circuitRegionSchema.safeParse({ x: 1, y: 2, w: 3, h: 4 }).success).toBe(true);
    expect(circuitRegionSchema.safeParse({ page: 2, x: 1.5, y: 2, w: 0, h: 0 }).success).toBe(true);
    expect(circuitRegionSchema.safeParse({ x: 1, y: 2, w: -1, h: 4 }).success).toBe(false);
    expect(circuitRegionSchema.safeParse({ page: 0, x: 1, y: 2, w: 3, h: 4 }).success).toBe(false);
    expect(circuitRegionSchema.safeParse({ x: 1, y: 2, w: Infinity, h: 4 }).success).toBe(false);
  });

  it("识别草稿不含 confidence，且拒绝草稿级多余字段", () => {
    const draft = {
      components: [
        {
          id: "draft-c1",
          ref: "U1",
          pins: [{ number: "1", netId: "draft-n1" }],
          origin: { by: "recognition", region: { x: 10, y: 20, w: 30, h: 40 } },
        },
      ],
      nets: [
        {
          id: "draft-n1",
          name: "3V3",
          pinRefs: [{ componentId: "draft-c1", pin: "1" }],
          origin: { by: "recognition" },
        },
      ],
    };
    expect(circuitRecognitionDraftSchema.safeParse(draft).success).toBe(true);
    expect(circuitRecognitionDraftSchema.safeParse({ ...draft, blocks: [] }).success).toBe(true);

    const withConfidence = {
      ...draft,
      components: [{ ...draft.components[0], origin: { by: "recognition", confidence: 0.8 } }],
    };
    expect(circuitRecognitionDraftSchema.safeParse(withConfidence).success).toBe(false);

    for (const extra of [{ irId: "ir-1" }, { revision: 1 }, { stale: false }, { diagnostics: [] }])
      expect(circuitRecognitionDraftSchema.safeParse({ ...draft, ...extra }).success).toBe(false);

    const missingOrigin = {
      ...draft,
      components: [{ id: "draft-c1", ref: "U1", pins: [] }],
    };
    expect(circuitRecognitionDraftSchema.safeParse(missingOrigin).success).toBe(false);
  });

  it("摘要与低置信度计数使用唯一的阈值来源", () => {
    const ir = buildIr();
    expect(countLowConfidenceElements(ir)).toBe(1);
    expect(isLowConfidence({ by: "recognition", confidence: CIRCUIT_LOW_CONFIDENCE_THRESHOLD })).toBe(false);
    expect(isLowConfidence({ by: "recognition", confidence: CIRCUIT_LOW_CONFIDENCE_THRESHOLD - 0.01 })).toBe(true);
    expect(countCircuitDiagnostics(ir.diagnostics)).toEqual({
      total: 2,
      error: 1,
      warning: 1,
      info: 0,
    });
    expect(toCircuitIrSummary(ir)).toEqual({
      irId: "ir-1",
      revision: 2,
      source: ir.source,
      stale: false,
      componentCount: 2,
      netCount: 2,
      blockCount: 1,
      lowConfidenceCount: 1,
      diagnostics: { total: 2, error: 1, warning: 1, info: 0 },
    });
  });
});
