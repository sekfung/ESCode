import { createHash } from "node:crypto";
import { describe, expect, it } from "vitest";
import type { CircuitRecognitionDraft } from "@escode/shared/circuit";
import { createCircuitService } from "../../src/circuit/app/circuitService.js";
import type { CircuitChangedEvent } from "../../src/circuit/contract.js";
import type { CircuitServicePorts, StoredCircuit } from "../../src/circuit/app/ports.js";
import type { CircuitVectorEvidence } from "../../src/circuit/domain/recognitionEvidence.js";

/**
 * `circuit_submit_recognition` 的服务行为（docs/circuit/README.md §2 规则 2/3、§3.2、§3.4、§5）：
 * 来源判定与 unsupported_format、内容哈希派生的 irId、同一内容的前置条件（baseRevision）、
 * 内容变化 = 新 IR（旧修正不迁移）、持久化完成之后才发事件。
 */

const PDF = new Uint8Array(Buffer.from("%PDF-1.4 recognition test", "latin1"));
const OTHER_PDF = new Uint8Array(Buffer.from("%PDF-1.4 recognition test (v2)", "latin1"));
const PNG = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x01]);
const DRAFT: CircuitRecognitionDraft = {
  components: [
    {
      id: "draft-u1",
      ref: "U1",
      value: "MPU-6050",
      pins: [{ number: "1", name: "VDD", netId: "draft-3v3" }],
      origin: { by: "recognition", region: { x: 40, y: 50, w: 60, h: 40 } },
    },
  ],
  nets: [
    {
      id: "draft-3v3",
      name: "3V3",
      pinRefs: [{ componentId: "draft-u1", pin: "1" }],
      origin: { by: "recognition" },
    },
  ],
};

function createHarness(options: { evidence?: CircuitVectorEvidence }) {
  const files = new Map<string, Uint8Array>([
    ["recognition/board.pdf", PDF],
    ["recognition/board.png", PNG],
  ]);
  const records = new Map<string, StoredCircuit>();
  const timeline: string[] = [];
  const events: CircuitChangedEvent[] = [];
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
        timeline.push(`write:${record.irId}:${record.ir.revision}`);
        records.set(storeKey(identityKey, sourcePath), structuredClone(record));
      },
    },
    source: {
      async readText({ relativePath }) {
        throw new Error(`not_found: ${relativePath}`);
      },
      async readBytes({ relativePath }) {
        const bytes = files.get(relativePath);
        if (bytes === undefined) throw new Error(`not_found: ${relativePath}`);
        return bytes;
      },
      async exists({ relativePath }) {
        return files.has(relativePath);
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
      async write() {
        throw new Error("io_error: 识别不写快照");
      },
    },
    vector: {
      async extract() {
        return options.evidence;
      },
    },
  };
  const service = createCircuitService({ ports, now: () => "2026-10-12T00:00:00.000Z" });
  service.onChanged((event) => {
    events.push(event);
    timeline.push(`event:${event.revision}:${event.stale ? "stale" : "fresh"}`);
  });
  return {
    service,
    records,
    timeline,
    events,
    replaceBytes: (relativePath: string, bytes: Uint8Array) => files.set(relativePath, bytes),
  };
}

const pdfParams = { workspacePath: "/ws", path: "recognition/board.pdf" };

describe("submitRecognition 的来源判定与 IR 标识（§3.2、§5）", () => {
  it("矢量 PDF：文本证据命中的元件记 1，来源 kind = pdf_vector，事件在持久化之后", async () => {
    const harness = createHarness({
      evidence: { pageCount: 1, texts: [{ page: 1, text: "U1", x: 60, y: 70 }], segments: [] },
    });
    const view = await harness.service.submitRecognition({ ...pdfParams, draft: DRAFT });

    expect(view.summary.revision).toBe(1);
    expect(view.summary.source).toEqual({
      kind: "pdf_vector",
      path: "recognition/board.pdf",
      contentHash: createHash("sha256").update(PDF).digest("hex"),
    });
    expect(view.summary.componentCount).toBe(1);
    expect(view.summary.netCount).toBe(1);
    // U1 命中 PDF 文本 → parser 1；3V3 只有一个引脚 → 0.8（高于低置信度阈值 0.7，还不算低置信度）。
    expect(view.summary.lowConfidenceCount).toBe(0);
    expect(harness.timeline).toEqual([`write:${view.summary.irId}:1`, "event:1:fresh"]);
  });

  it("页内只有位图的 PDF 与截图都按 image 处理，识别元素永不等于 1", async () => {
    const raster = createHarness({ evidence: undefined });
    const rasterView = await raster.service.submitRecognition({ ...pdfParams, draft: DRAFT });
    expect(rasterView.summary.source.kind).toBe("image");
    expect(rasterView.summary.lowConfidenceCount).toBe(0);

    const image = createHarness({ evidence: undefined });
    const imageView = await image.service.submitRecognition({
      workspacePath: "/ws",
      path: "recognition/board.png",
      draft: DRAFT,
    });
    expect(imageView.summary.source.kind).toBe("image");
    expect(imageView.summary.source.contentHash).toBe(
      createHash("sha256").update(PNG).digest("hex"),
    );
    const component = (
      await image.service.query({
        workspacePath: "/ws",
        irId: imageView.summary.irId,
        by: "ref",
        ref: "U1",
      })
    ).items[0]!;
    expect(component.origin).toEqual({
      by: "recognition",
      confidence: 0.9,
      region: { x: 40, y: 50, w: 60, h: 40 },
    });
  });

  it("扣分后低于阈值的元素计入 lowConfidenceCount 与诊断（§2 规则 4）", async () => {
    const harness = createHarness({ evidence: undefined });
    const view = await harness.service.submitRecognition({
      ...pdfParams,
      draft: {
        components: [
          { id: "a", ref: "R1", pins: [], origin: { by: "recognition" } },
          { id: "b", ref: "R1", pins: [], origin: { by: "recognition" } },
        ],
        nets: [],
      },
    });
    // 两个 R1 都带位号重复（−0.3）→ 0.6 < 0.7，两个都算低置信度。
    expect(view.summary.lowConfidenceCount).toBe(2);
    expect(view.summary.diagnostics).toMatchObject({ total: 1, error: 1 });
    expect(view.diagnostics[0]).toMatchObject({
      code: "duplicate-ref",
      componentIds: ["recognition:R1", "recognition:R1#2"],
    });
  });

  it("不是图片/PDF 的来源直接报 unsupported_format，不读文件", async () => {
    const harness = createHarness({ evidence: undefined });
    await expect(
      harness.service.submitRecognition({
        workspacePath: "/ws",
        path: "hardware/board.kicad_sch",
        draft: DRAFT,
      }),
    ).rejects.toMatchObject({ code: "unsupported_format" });
    expect(harness.timeline).toEqual([]);
  });

  it("源文件不存在时把端口的 not_found 原样带出", async () => {
    const harness = createHarness({ evidence: undefined });
    await expect(
      harness.service.submitRecognition({
        workspacePath: "/ws",
        path: "recognition/missing.png",
        draft: DRAFT,
      }),
    ).rejects.toThrow(/not_found/u);
  });
});

describe("submitRecognition 的前置条件与内容哈希（§3.2、§3.4）", () => {
  it("同一份内容重新提交沿用 parse 的前置条件：invalid_ir → stale_revision → revision+1", async () => {
    const harness = createHarness({ evidence: undefined });
    const first = await harness.service.submitRecognition({ ...pdfParams, draft: DRAFT });
    const irId = first.summary.irId;

    await expect(
      harness.service.submitRecognition({ ...pdfParams, draft: DRAFT }),
    ).rejects.toMatchObject({ code: "invalid_ir" });
    await expect(
      harness.service.submitRecognition({ ...pdfParams, draft: DRAFT, baseRevision: 7 }),
    ).rejects.toMatchObject({ code: "stale_revision" });

    const second = await harness.service.submitRecognition({
      ...pdfParams,
      draft: DRAFT,
      baseRevision: 1,
    });
    expect(second.summary.irId).toBe(irId);
    expect(second.summary.revision).toBe(2);
    expect(harness.timeline.at(-1)).toBe("event:2:fresh");
  });

  it("内容一变就是一份新 IR：新 irId、revision 从 1 起、旧修正不迁移", async () => {
    const harness = createHarness({ evidence: undefined });
    const first = await harness.service.submitRecognition({ ...pdfParams, draft: DRAFT });
    // 先落一条人工修正，验证它不会被带进新内容的 IR。
    await harness.service.correct({
      workspacePath: "/ws",
      irId: first.summary.irId,
      baseRevision: 1,
      ops: [{ op: "update-component", componentId: "recognition:U1", value: "MPU-6050 (fixed)" }],
      author: { kind: "user" },
    });
    expect(harness.records.get("/ws\u0000recognition/board.pdf")!.corrections).toHaveLength(1);

    harness.replaceBytes("recognition/board.pdf", OTHER_PDF);
    const second = await harness.service.submitRecognition({ ...pdfParams, draft: DRAFT });
    expect(second.summary.irId).not.toBe(first.summary.irId);
    expect(second.summary.revision).toBe(1);
    const record = harness.records.get("/ws\u0000recognition/board.pdf")!;
    expect(record.irId).toBe(second.summary.irId);
    expect(record.corrections).toEqual([]);
    // 旧 irId 已经不属于这个 workspace：不会被 findByIrId 找回。
    await expect(
      harness.service.get({ workspacePath: "/ws", irId: first.summary.irId }),
    ).rejects.toMatchObject({ code: "not_found" });
    expect((await harness.service.findBySource(pdfParams))?.irId).toBe(second.summary.irId);
  });

  it("同一份内容重新识别时按 id 重新套用已有修正", async () => {
    const harness = createHarness({ evidence: undefined });
    const first = await harness.service.submitRecognition({ ...pdfParams, draft: DRAFT });
    await harness.service.correct({
      workspacePath: "/ws",
      irId: first.summary.irId,
      baseRevision: 1,
      ops: [{ op: "update-component", componentId: "recognition:U1", value: "MPU-6050 (fixed)" }],
      author: { kind: "user" },
    });

    const second = await harness.service.submitRecognition({
      ...pdfParams,
      draft: DRAFT,
      baseRevision: 2,
    });
    expect(second.summary.revision).toBe(3);
    const component = (
      await harness.service.query({
        workspacePath: "/ws",
        irId: second.summary.irId,
        by: "ref",
        ref: "U1",
      })
    ).items[0]!;
    expect(component.value).toBe("MPU-6050 (fixed)");
    expect(harness.records.get("/ws\u0000recognition/board.pdf")!.corrections).toHaveLength(1);
  });
});
