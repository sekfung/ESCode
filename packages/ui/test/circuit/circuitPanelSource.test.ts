import { describe, expect, it } from "vitest";
import type { CircuitIR } from "@escode/shared/circuit";
import {
  circuitPanelFileKind,
  describeCircuitError,
  isStaleRevisionError,
  parseCircuitSnapshot,
  shouldRefreshCircuitPanel,
} from "@/circuit/domain/circuitPanelSource.js";

/** 面板入口分类、导出快照解析与事件刷新判据（规格 §3.3、§3.4、§8）。 */

const ir: CircuitIR = {
  schemaVersion: 1,
  irId: "ir:1",
  revision: 3,
  source: { kind: "kicad_sch", path: "hardware/board.kicad_sch", contentHash: "a".repeat(64) },
  stale: false,
  components: [],
  nets: [],
  blocks: [],
  diagnostics: [],
};

describe("面板入口分类", () => {
  it.each([
    ["hardware/board.kicad_sch", "kicad_sch"],
    ["hardware/BOARD.KICAD_PCB", "kicad_pcb"],
    ["out/board.circuit.json", "snapshot"],
    ["docs/schematic.pdf", "recognition"],
    ["docs/photo.png", "recognition"],
    ["src/main.ts", "unsupported"],
  ] as const)("%s → %s", (path, expected) => {
    expect(circuitPanelFileKind(path)).toBe(expected);
  });

  it("没有路径时是 unsupported，不猜格式", () => {
    expect(circuitPanelFileKind(undefined)).toBe("unsupported");
    expect(circuitPanelFileKind("")).toBe("unsupported");
  });
});

describe("导出快照解析（只读派生产物）", () => {
  it("读取快照里的 ir 字段", () => {
    const text = JSON.stringify({ derived: true, generatedBy: "circuit-service", ir });
    expect(parseCircuitSnapshot(text)).toMatchObject({ ok: true, ir: { irId: "ir:1" } });
  });

  it.each([
    ["not json", "invalid"],
    [JSON.stringify({ derived: true }), "invalid"],
    [JSON.stringify({ ir: { ...ir, revision: -1 } }), "invalid"],
    [undefined, "unreadable"],
    [null, "unreadable"],
  ] as const)("不合法内容不展示：%s → %s", (text, reason) => {
    expect(parseCircuitSnapshot(text)).toEqual({ ok: false, reason });
  });
});

describe("事件刷新判据", () => {
  const known = { irId: "ir:1", revision: 3, stale: false };

  it("revision 前进要刷新，落后或重复的事件忽略", () => {
    expect(shouldRefreshCircuitPanel(known, { irId: "ir:1", revision: 4, stale: false })).toBe(true);
    expect(shouldRefreshCircuitPanel(known, { irId: "ir:1", revision: 3, stale: false })).toBe(false);
    expect(shouldRefreshCircuitPanel(known, { irId: "ir:1", revision: 2, stale: false })).toBe(false);
  });

  it("revision 不变但 stale 翻转也要刷新（源文件变化的语义）", () => {
    expect(shouldRefreshCircuitPanel(known, { irId: "ir:1", revision: 3, stale: true })).toBe(true);
    expect(
      shouldRefreshCircuitPanel({ ...known, stale: true }, { irId: "ir:1", revision: 3, stale: false }),
    ).toBe(true);
  });

  it("其它 irId 的事件不触发本面板刷新", () => {
    expect(shouldRefreshCircuitPanel(known, { irId: "ir:2", revision: 9, stale: true })).toBe(false);
    expect(shouldRefreshCircuitPanel(null, { irId: "ir:1", revision: 4, stale: true })).toBe(false);
  });
});

describe("服务错误映射", () => {
  it("保留服务错误码与消息", () => {
    const error = Object.assign(new Error("当前 revision 为 4"), { code: "stale_revision" });
    expect(describeCircuitError(error)).toEqual({
      code: "stale_revision",
      message: "当前 revision 为 4",
    });
    expect(isStaleRevisionError(error)).toBe(true);
  });

  it("没有错误码时按 io_error 处理", () => {
    expect(describeCircuitError(new Error("boom"))).toEqual({ message: "boom" });
    expect(describeCircuitError("boom")).toEqual({ message: "boom" });
    expect(isStaleRevisionError(new Error("boom"))).toBe(false);
  });
});
