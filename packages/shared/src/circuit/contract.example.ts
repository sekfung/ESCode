import { toCircuitIrSummary } from "./ir.js";
import type { CircuitIR, CircuitToolRequest, CircuitToolResponse } from "./contract.js";

const EXAMPLE_CONTENT_HASH = "0".repeat(64);

/** 解析结果示例：KiCad 来源元素 confidence 恒为 1，网络有一条单引脚诊断。 */
export const exampleCircuitIr: CircuitIR = {
  schemaVersion: 1,
  irId: "circuit-example",
  revision: 3,
  source: {
    kind: "kicad_sch",
    path: "hardware/example/example.kicad_sch",
    contentHash: EXAMPLE_CONTENT_HASH,
  },
  stale: false,
  components: [
    {
      id: "6f0a6d0e-0000-4000-8000-000000000001",
      ref: "U1",
      value: "STM32F103C8T6",
      footprint: "Package_QFP:LQFP-48_7x7mm_P0.5mm",
      pins: [{ number: "1", name: "VBAT", netId: "net-3v3" }],
      origin: { by: "parser", confidence: 1 },
    },
    {
      id: "6f0a6d0e-0000-4000-8000-000000000002",
      ref: "C1",
      value: "100nF",
      footprint: "Capacitor_SMD:C_0402_1005Metric",
      pins: [
        { number: "1", netId: "net-3v3" },
        { number: "2", netId: "net-gnd" },
      ],
      origin: { by: "parser", confidence: 1 },
    },
  ],
  nets: [
    {
      id: "net-3v3",
      name: "3V3",
      pinRefs: [
        { componentId: "6f0a6d0e-0000-4000-8000-000000000001", pin: "1" },
        { componentId: "6f0a6d0e-0000-4000-8000-000000000002", pin: "1" },
      ],
      origin: { by: "parser", confidence: 1 },
    },
    {
      id: "net-gnd",
      name: "GND",
      pinRefs: [{ componentId: "6f0a6d0e-0000-4000-8000-000000000002", pin: "2" }],
      origin: { by: "parser", confidence: 1 },
    },
  ],
  blocks: [
    {
      id: "block-mcu",
      name: "MCU 最小系统",
      componentIds: [
        "6f0a6d0e-0000-4000-8000-000000000001",
        "6f0a6d0e-0000-4000-8000-000000000002",
      ],
      origin: { by: "correction", confidence: 1, author: { kind: "user" } },
    },
  ],
  diagnostics: [
    {
      code: "single-pin-net",
      severity: "warning",
      message: "网络 GND 只连了一个引脚",
      netIds: ["net-gnd"],
    },
  ],
};

export const exampleCircuitSummary = toCircuitIrSummary(exampleCircuitIr);

export const exampleParseRequest: CircuitToolRequest<"parse"> = {
  path: "hardware/example/example.kicad_sch",
  baseRevision: 3,
};

export const exampleGetResponse: CircuitToolResponse<"get"> = {
  ok: true,
  summary: exampleCircuitSummary,
  diagnostics: exampleCircuitIr.diagnostics,
  truncated: false,
};

export const exampleExportFailure: CircuitToolResponse<"export"> = {
  ok: false,
  error: { code: "stale_revision", message: "IR 已被其他写入更新，请重新读取后再导出" },
};
