import type {
  CircuitChangedEvent,
  CircuitCorrectParams,
  CircuitIrView,
  CircuitLayout,
  CircuitParseParams,
  CircuitQueryResult,
} from "./contract.js";
import type { CircuitCorrectionRecord } from "./domain/circuitCorrections.js";

/** contract 用法示例（与 contract.test.ts 一起保证契约的消费方式可编译、可断言）。 */

export const exampleCircuitParseParams: CircuitParseParams = {
  workspacePath: "C:/work/board",
  path: "hardware/board.kicad_sch",
};

export const exampleCircuitCorrectParams: CircuitCorrectParams = {
  workspacePath: "C:/work/board",
  irId: "ir:0123456789abcdef",
  baseRevision: 3,
  ops: [
    { op: "update-component", componentId: "6f0a6d0e-0000-4000-8000-000000000001", ref: "U2" },
    { op: "connect", componentId: "6f0a6d0e-0000-4000-8000-000000000001", pin: "7", netName: "I2C1_SDA" },
  ],
  author: { kind: "user" },
};

export const exampleCircuitIrView: CircuitIrView = {
  summary: {
    irId: "ir:0123456789abcdef",
    revision: 3,
    source: {
      kind: "kicad_sch",
      path: "hardware/board.kicad_sch",
      contentHash: "0".repeat(64),
    },
    stale: false,
    componentCount: 24,
    netCount: 41,
    blockCount: 3,
    lowConfidenceCount: 0,
    diagnostics: { total: 1, error: 0, warning: 1, info: 0 },
  },
  diagnostics: [
    {
      code: "single-pin-net",
      severity: "warning",
      message: "网络 Net-(TP1-Pad1) 只连了一个引脚",
      netIds: ["net:Net-(TP1-Pad1)"],
    },
  ],
  truncated: false,
};

export const exampleCircuitLayout: CircuitLayout = {
  irId: "ir:0123456789abcdef",
  revision: 3,
  sheets: [
    {
      path: "",
      name: "board",
      page: { width: 297, height: 210 },
      symbols: [
        {
          elementId: "6f0a6d0e-0000-4000-8000-000000000001",
          libId: "Device:R",
          unit: 1,
          x: 100,
          y: 100,
          rotation: 0,
          shapes: [{ kind: "rectangle", points: [{ x: 98.984, y: 97.46 }, { x: 101.016, y: 102.54 }], filled: false }],
          pins: [
            { number: "1", connection: { x: 100, y: 96.19 }, body: { x: 100, y: 97.46 } },
            { number: "2", connection: { x: 100, y: 103.81 }, body: { x: 100, y: 102.54 } },
          ],
          texts: [
            { kind: "reference", text: "R1", x: 102, y: 99, rotation: 0 },
            { kind: "value", text: "10k", x: 102, y: 101, rotation: 0 },
          ],
        },
      ],
      wires: [{ points: [{ x: 100, y: 103.81 }, { x: 100, y: 106.19 }] }],
      junctions: [],
      labels: [{ text: "MID", x: 100, y: 105, rotation: 0, kind: "local" }],
    },
  ],
};

export const exampleCircuitChangedEvent: CircuitChangedEvent = {
  irId: "ir:0123456789abcdef",
  revision: 4,
  stale: false,
};

export const exampleCircuitCorrectionRecord: CircuitCorrectionRecord = {
  revision: 4,
  appliedAt: "2026-10-12T09:00:00.000Z",
  author: { kind: "agent", sessionId: "session-1" },
  target: { kind: "element", id: "6f0a6d0e-0000-4000-8000-000000000001" },
  op: { op: "update-component", componentId: "6f0a6d0e-0000-4000-8000-000000000001", ref: "U2" },
};

export const exampleCircuitQueryResult: CircuitQueryResult = {
  by: "net",
  items: [
    {
      id: "net:MID",
      name: "MID",
      pinRefs: [
        { componentId: "6f0a6d0e-0000-4000-8000-000000000001", pin: "2" },
        { componentId: "6f0a6d0e-0000-4000-8000-000000000002", pin: "1" },
      ],
      origin: { by: "parser", confidence: 1 },
    },
  ],
};
