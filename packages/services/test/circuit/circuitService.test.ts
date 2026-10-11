import { createHash } from "node:crypto";
import { describe, expect, it } from "vitest";
import type { CircuitChangedEvent } from "../../src/circuit/contract.js";
import { createCircuitService } from "../../src/circuit/app/circuitService.js";
import type { CircuitServicePorts, StoredCircuit } from "../../src/circuit/app/ports.js";

/**
 * CircuitService 行为与事件顺序（规格 §3.3、§3.4、§10.2）：
 * 这里用内存端口记录「持久化 → 事件」的时间线，确保事件只在写入完成后发出。
 */

const R1 = `(symbol (lib_id "Device:R") (at 100 100 0) (unit 1) (exclude_from_sim no) (in_bom yes) (on_board yes) (dnp no) (uuid r1-uuid)
  (property "Reference" "R1" (at 102 99 0) (effects (font (size 1.27 1.27))))
  (property "Value" "10k" (at 102 101 0) (effects (font (size 1.27 1.27))))
  (pin "1" (uuid r1-pin1)) (pin "2" (uuid r1-pin2))
  (instances (project "demo" (path "/root-uuid" (reference "R1") (unit 1)))))`;

const R2 = `(symbol (lib_id "Device:R") (at 100 110 0) (unit 1) (exclude_from_sim no) (in_bom yes) (on_board yes) (dnp no) (uuid r2-uuid)
  (property "Reference" "R2" (at 102 109 0) (effects (font (size 1.27 1.27))))
  (property "Value" "10k" (at 102 111 0) (effects (font (size 1.27 1.27))))
  (pin "1" (uuid r2-pin1)) (pin "2" (uuid r2-pin2))
  (instances (project "demo" (path "/root-uuid" (reference "R2") (unit 1)))))`;

const WIRE = `(wire (pts (xy 100 103.81) (xy 100 106.19)) (stroke (width 0) (type default)) (uuid w1))
  (label "MID" (at 100 105 0) (effects (font (size 1.27 1.27))) (uuid l1))`;

function schematic(symbols: string): string {
  return `(kicad_sch (version 20231120) (generator eeschema) (uuid root-uuid) (paper "A4")
  (lib_symbols
    (symbol "Device:R" (pin_numbers hide) (pin_names (offset 0)) (exclude_from_sim no) (in_bom yes) (on_board yes)
      (property "Reference" "R" (at 2.032 0 90) (effects (font (size 1.27 1.27))))
      (property "Value" "R" (at 0 0 90) (effects (font (size 1.27 1.27))))
      (symbol "R_0_1" (rectangle (start -1.016 -2.54) (end 1.016 2.54)))
      (symbol "R_1_1"
        (pin passive line (at 0 3.81 270) (length 1.27) (name "~" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27)))))
        (pin passive line (at 0 -3.81 90) (length 1.27) (name "~" (effects (font (size 1.27 1.27)))) (number "2" (effects (font (size 1.27 1.27))))))))
${symbols}
)`;
}

function createHarness() {
  const files = new Map<string, string>();
  const records = new Map<string, StoredCircuit>();
  const snapshots = new Map<string, string>();
  const timeline: string[] = [];
  const events: CircuitChangedEvent[] = [];
  const watchers = new Set<() => void>();
  const storeKey = (identityKey: string, sourcePath: string) => `${identityKey}\u0000${sourcePath}`;
  const ports: CircuitServicePorts = {
    store: {
      async read({ identityKey, sourcePath }) {
        const record = records.get(storeKey(identityKey, sourcePath));
        return record ? structuredClone(record) : undefined;
      },
      async findByIrId({ identityKey, irId }) {
        for (const [key, record] of records)
          if (key.startsWith(`${identityKey}\u0000`) && record.irId === irId)
            return structuredClone(record);
        return undefined;
      },
      async write({ identityKey, sourcePath, record }) {
        timeline.push(`write:${record.ir.revision}:${record.ir.stale ? "stale" : "fresh"}`);
        records.set(storeKey(identityKey, sourcePath), structuredClone(record));
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
      async watch({ onChange }) {
        watchers.add(onChange);
        return {
          dispose: () => {
            watchers.delete(onChange);
          },
        };
      },
    },
    snapshot: {
      async write({ relativePath, content }) {
        timeline.push(`snapshot:${relativePath}`);
        snapshots.set(relativePath, content);
      },
    },
  };
  const service = createCircuitService({
    ports,
    now: () => "2026-10-12T00:00:00.000Z",
  });
  service.onChanged((event) => {
    events.push(event);
    timeline.push(`event:${event.revision}:${event.stale ? "stale" : "fresh"}`);
  });
  return {
    files,
    records,
    snapshots,
    timeline,
    events,
    service,
    touchSource: (path: string, text: string) => {
      files.set(path, text);
      for (const onChange of watchers) onChange();
    },
    storedRecord: (sourcePath: string) => records.get(storeKey("/ws", sourcePath)),
  };
}

const parseParams = { workspacePath: "/ws", path: "board.kicad_sch" };

describe("CircuitService 解析与事件顺序", () => {
  it("持久化完成后才发事件，事件里的 revision 一定能读到", async () => {
    const harness = createHarness();
    harness.files.set(parseParams.path, schematic(`${R1}\n  ${R2}\n  ${WIRE}`));

    const view = await harness.service.parse(parseParams);
    expect(view.summary.revision).toBe(1);
    expect(view.summary.componentCount).toBe(2);
    expect(view.summary.netCount).toBe(3);
    expect(harness.timeline).toEqual(["write:1:fresh", "event:1:fresh"]);

    const stored = harness.storedRecord(parseParams.path)!;
    expect(stored.ir.revision).toBe(1);
    expect(stored.ir.stale).toBe(false);
    expect(stored.layout?.revision).toBe(1);
  });

  it("已有 IR 时重新解析必须带 baseRevision，落后则拒绝 stale_revision", async () => {
    const harness = createHarness();
    harness.files.set(parseParams.path, schematic(`${R1}\n  ${R2}\n  ${WIRE}`));
    await harness.service.parse(parseParams);

    await expect(harness.service.parse(parseParams)).rejects.toThrow(/需要 baseRevision/u);
    await expect(harness.service.parse({ ...parseParams, baseRevision: 99 })).rejects.toThrow(
      /stale_revision/u,
    );
    expect(harness.timeline).toEqual(["write:1:fresh", "event:1:fresh"]);

    const view = await harness.service.parse({ ...parseParams, baseRevision: 1 });
    expect(view.summary.revision).toBe(2);
    expect(harness.timeline.at(-2)).toBe("write:2:fresh");
    expect(harness.timeline.at(-1)).toBe("event:2:fresh");
  });

  it("findBySource 返回已有 IR 的摘要，没有则 undefined", async () => {
    const harness = createHarness();
    expect(await harness.service.findBySource(parseParams)).toBeUndefined();
    harness.files.set(parseParams.path, schematic(`${R1}\n  ${R2}\n  ${WIRE}`));
    const view = await harness.service.parse(parseParams);
    expect(await harness.service.findBySource(parseParams)).toEqual({
      irId: view.summary.irId,
      revision: 1,
      stale: false,
    });
  });

  it("不支持的文件类型直接拒绝", async () => {
    const harness = createHarness();
    harness.files.set("notes.txt", "hello");
    await expect(
      harness.service.parse({ workspacePath: "/ws", path: "notes.txt" }),
    ).rejects.toThrow(/unsupported_format/u);
  });
});

describe("CircuitService 源文件变化与 stale", () => {
  it("源文件变化只翻转 stale（revision 不变），并且只发一次事件", async () => {
    const harness = createHarness();
    harness.files.set(parseParams.path, schematic(`${R1}\n  ${R2}\n  ${WIRE}`));
    await harness.service.parse(parseParams);

    harness.touchSource(parseParams.path, schematic(`${R1}\n  ${R2}\n  ${WIRE}\n  (junction (at 100 105) (diameter 0) (color 0 0 0 0) (uuid j1))`));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(harness.timeline.slice(-2)).toEqual(["write:1:stale", "event:1:stale"]);
    expect(harness.events.at(-1)).toMatchObject({ revision: 1, stale: true });

    // 已在 stale 状态：再次变化不再重复发事件。
    harness.touchSource(parseParams.path, schematic(`${R1}\n  ${R2}\n  ${WIRE}`));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(harness.timeline.filter((entry) => entry === "event:1:stale")).toHaveLength(1);

    const view = await harness.service.parse({ ...parseParams, baseRevision: 1 });
    expect(view.summary.stale).toBe(false);
    expect(view.summary.revision).toBe(2);
  });
});

describe("CircuitService 修正", () => {
  it("写入必须带 baseRevision；并发写入只有一个成功", async () => {
    const harness = createHarness();
    harness.files.set(parseParams.path, schematic(`${R1}\n  ${R2}\n  ${WIRE}`));
    const parsed = await harness.service.parse(parseParams);

    const [first, second] = await Promise.allSettled([
      harness.service.correct({
        workspacePath: "/ws",
        irId: parsed.summary.irId,
        baseRevision: 1,
        ops: [{ op: "update-component", componentId: "r1-uuid", value: "4k7" }],
        author: { kind: "user" },
      }),
      harness.service.correct({
        workspacePath: "/ws",
        irId: parsed.summary.irId,
        baseRevision: 1,
        ops: [{ op: "update-component", componentId: "r2-uuid", value: "1k" }],
        author: { kind: "user" },
      }),
    ]);
    expect([first.status, second.status].sort()).toEqual(["fulfilled", "rejected"]);
    const rejected = (first.status === "rejected" ? first : second) as PromiseRejectedResult;
    expect(String(rejected.reason)).toMatch(/stale_revision/u);

    const stored = harness.storedRecord(parseParams.path)!;
    expect(stored.ir.revision).toBe(2);
    expect(
      stored.ir.components.filter((component) => component.value !== "10k"),
    ).toHaveLength(1);
    expect(stored.corrections).toHaveLength(1);
    expect(stored.corrections[0]).toMatchObject({
      revision: 2,
      appliedAt: "2026-10-12T00:00:00.000Z",
      author: { kind: "user" },
    });
    // 修正后重算诊断：R1.2 与 R2.1 的 MID 网络仍在，没有新增未知引用。
    expect(stored.ir.diagnostics.filter((entry) => entry.severity === "error")).toEqual([]);
  });

  it("修正记录在重新解析后按 KiCad UUID 重新套用，套不上的标 orphaned", async () => {
    const harness = createHarness();
    harness.files.set(parseParams.path, schematic(`${R1}\n  ${R2}\n  ${WIRE}`));
    const parsed = await harness.service.parse(parseParams);

    await harness.service.correct({
      workspacePath: "/ws",
      irId: parsed.summary.irId,
      baseRevision: 1,
      ops: [
        { op: "update-component", componentId: "r1-uuid", ref: "R101", value: "47k" },
        { op: "connect", componentId: "r2-uuid", pin: "2", netName: "SENSE" },
        { op: "add-block", name: "分压", componentIds: ["r1-uuid", "r2-uuid"] },
      ],
      author: { kind: "user" },
    });

    // 源文件变化后重新解析：R1 仍在（UUID 不变）→ 修正保留。
    harness.files.set(parseParams.path, schematic(`${R1}\n  ${R2}\n  ${WIRE}\n  `));
    const reparsed = await harness.service.parse({ ...parseParams, baseRevision: 2 });
    expect(reparsed.summary.revision).toBe(3);
    const components = (await harness.service.get({
      workspacePath: "/ws",
      irId: reparsed.summary.irId,
    })).summary;
    expect(components.componentCount).toBe(2);
    const queried = await harness.service.query({
      workspacePath: "/ws",
      irId: reparsed.summary.irId,
      by: "ref",
      ref: "R101",
    });
    expect(queried.by === "ref" && queried.items[0]?.value).toBe("47k");
    expect(harness.storedRecord(parseParams.path)!.corrections.some((entry) => entry.orphaned)).toBe(
      false,
    );

    // R1 从源文件里消失 → 对应修正套不上，标记 orphaned；列表里能读到，且只能删除 orphaned 记录。
    harness.files.set(parseParams.path, schematic(`${R2}\n  ${WIRE}`));
    const afterRemoval = await harness.service.parse({ ...parseParams, baseRevision: 3 });
    expect(afterRemoval.summary.componentCount).toBe(1);
    const corrections = await harness.service.listCorrections({
      workspacePath: "/ws",
      irId: afterRemoval.summary.irId,
    });
    const orphaned = corrections.filter((entry) => entry.record.orphaned);
    expect(orphaned).toHaveLength(2);
    expect(orphaned[0]!.record.target).toMatchObject({ kind: "element", id: "r1-uuid" });

    const kept = corrections.find((entry) => !entry.record.orphaned)!;
    await expect(
      harness.service.dropCorrection({
        workspacePath: "/ws",
        irId: afterRemoval.summary.irId,
        baseRevision: 4,
        index: kept.index,
      }),
    ).rejects.toThrow(/orphaned/u);
    await harness.service.dropCorrection({
      workspacePath: "/ws",
      irId: afterRemoval.summary.irId,
      baseRevision: 4,
      index: orphaned[0]!.index,
    });
    const remaining = await harness.service.listCorrections({
      workspacePath: "/ws",
      irId: afterRemoval.summary.irId,
    });
    expect(remaining.filter((entry) => entry.record.orphaned)).toHaveLength(1);
  });

  it("修正引用不存在的元素时报 invalid_ir，不改动任何状态", async () => {
    const harness = createHarness();
    harness.files.set(parseParams.path, schematic(`${R1}\n  ${R2}\n  ${WIRE}`));
    const parsed = await harness.service.parse(parseParams);
    const before = harness.timeline.length;

    await expect(
      harness.service.correct({
        workspacePath: "/ws",
        irId: parsed.summary.irId,
        baseRevision: 1,
        ops: [{ op: "update-component", componentId: "missing-uuid", value: "x" }],
        author: { kind: "agent", sessionId: "s-1" },
      }),
    ).rejects.toThrow(/invalid_ir/u);
    expect(harness.timeline.length).toBe(before);
    expect(harness.storedRecord(parseParams.path)!.ir.revision).toBe(1);
  });
});

describe("CircuitService 导出快照", () => {
  it("写入带派生产物标记的快照，且不改动 IR 与 revision", async () => {
    const harness = createHarness();
    harness.files.set(parseParams.path, schematic(`${R1}\n  ${R2}\n  ${WIRE}`));
    const parsed = await harness.service.parse(parseParams);

    const result = await harness.service.exportSnapshot({
      workspacePath: "/ws",
      irId: parsed.summary.irId,
      path: "out/board.circuit.json",
    });
    expect(result).toEqual({ path: "out/board.circuit.json" });
    const snapshot = JSON.parse(harness.snapshots.get("out/board.circuit.json")!) as {
      derived: boolean;
      irId: string;
      revision: number;
      ir: { revision: number };
    };
    expect(snapshot.derived).toBe(true);
    expect(snapshot.irId).toBe(parsed.summary.irId);
    expect(snapshot.revision).toBe(1);
    expect(snapshot.ir.revision).toBe(1);
    // 导出是只读派生：不写 store、不发事件、不产生第二次持久化。
    expect(harness.timeline.filter((entry) => entry.startsWith("snapshot:"))).toEqual([
      "snapshot:out/board.circuit.json",
    ]);
    expect(harness.timeline.filter((entry) => entry.startsWith("event:"))).toEqual(["event:1:fresh"]);
    expect(harness.storedRecord(parseParams.path)!.ir.revision).toBe(1);
  });

  it("目标不是 .circuit.json 或 irId 不在本 workspace 时拒绝", async () => {
    const harness = createHarness();
    harness.files.set(parseParams.path, schematic(`${R1}\n  ${R2}\n  ${WIRE}`));
    const parsed = await harness.service.parse(parseParams);

    await expect(
      harness.service.exportSnapshot({
        workspacePath: "/ws",
        irId: parsed.summary.irId,
        path: "out/board.json",
      }),
    ).rejects.toThrow(/unsupported_format/u);
    // irId 由 (identityKey, sourcePath) 派生且无法反解：服务只按请求方 workspace 反查（见 locationFor）。
    await expect(
      harness.service.exportSnapshot({
        workspacePath: "/ws",
        irId: "ir:unknown",
        path: "out/board.circuit.json",
      }),
    ).rejects.toThrow(/not_found/u);
    expect(harness.snapshots.size).toBe(0);
  });
});

describe("CircuitService 查询", () => {
  it("按 ref / net / trace 查询，游标与查询不匹配时拒绝", async () => {
    const harness = createHarness();
    harness.files.set(parseParams.path, schematic(`${R1}\n  ${R2}\n  ${WIRE}`));
    const parsed = await harness.service.parse(parseParams);
    const base = { workspacePath: "/ws", irId: parsed.summary.irId };

    const byRef = await harness.service.query({ ...base, by: "ref", ref: "R1" });
    expect(byRef.by === "ref" && byRef.items[0]?.pins[0]).toMatchObject({ number: "1" });

    const byNet = await harness.service.query({ ...base, by: "net", netName: "MID" });
    expect(byNet.by === "net" && byNet.items[0]?.pinRefs).toEqual([
      { componentId: "r1-uuid", pin: "2" },
      { componentId: "r2-uuid", pin: "1" },
    ]);

    const trace = await harness.service.query({ ...base, by: "trace", componentId: "r1-uuid", pin: "2" });
    expect(trace.by === "trace" && trace.items[0]?.netIds).toEqual(["net:MID"]);
    expect(trace.by === "trace" && trace.items[0]?.components.map((item) => item.ref)).toEqual([
      "R1",
      "R2",
    ]);

    const firstPage = await harness.service.query({ ...base, by: "ref", ref: "R1" });
    if (firstPage.by === "ref" && firstPage.nextCursor) {
      await expect(
        harness.service.query({ ...base, by: "ref", ref: "R2", cursor: firstPage.nextCursor }),
      ).rejects.toThrow(/cursor/u);
    }
  });

  // 面板的 BOM 与网络表要读全量元素（§8）：省略过滤条件 = 列出该类全部，读的仍是 IR。
  it("省略 ref / net 过滤时列出全部器件与网络", async () => {
    const harness = createHarness();
    harness.files.set(parseParams.path, schematic(`${R1}\n  ${R2}\n  ${WIRE}`));
    const parsed = await harness.service.parse(parseParams);
    const base = { workspacePath: "/ws", irId: parsed.summary.irId };

    const allRefs = await harness.service.query({ ...base, by: "ref" });
    expect(allRefs.by === "ref" && allRefs.items.map((item) => item.ref)).toEqual(["R1", "R2"]);
    expect(allRefs.by === "ref" && allRefs.items[0]?.value).toBe("10k");
    expect(allRefs.by === "ref" && allRefs.nextCursor).toBeUndefined();

    const allNets = await harness.service.query({ ...base, by: "net" });
    expect(allNets.by === "net" && allNets.items.map((item) => item.name ?? item.id)).toContain("MID");

    // 省略过滤的游标仍与查询签名绑定：换成带过滤的查询会被拒绝。
    const page = await harness.service.query({ ...base, by: "net" });
    if (page.by === "net" && page.nextCursor) {
      await expect(
        harness.service.query({ ...base, by: "net", netName: "MID", cursor: page.nextCursor }),
      ).rejects.toThrow(/cursor/u);
    }
  });
});
