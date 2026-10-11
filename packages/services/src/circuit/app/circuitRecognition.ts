import { countLowConfidenceElements, type CircuitSourceKind } from "@escode/shared/circuit";
import { createServiceLogger } from "../../logger/serviceLogger.js";
import type { CircuitSubmitRecognitionParams } from "../contract.js";
import { reapplyCircuitCorrections } from "../domain/circuitCorrections.js";
import { assembleCircuitIr } from "../domain/irBuild.js";
import { buildRecognitionElements } from "../domain/recognitionDraft.js";
import { isVectorEvidence } from "../domain/recognitionEvidence.js";
import { circuitContentIrIdOf, type CircuitLocation } from "./circuitLocation.js";
import { requireReparseRevision } from "./correctionMaintenance.js";
import type { CircuitSourcePort, CircuitVectorPort, StoredCircuit } from "./ports.js";
import { detectRecognitionSourceKind } from "./sourceExtraction.js";

const log = createServiceLogger("circuit");

/**
 * `circuit_submit_recognition` 的编排（§5、§3.2、§2 规则 2/3）：
 * 读源文件字节 → 判来源（矢量 PDF / 图片）→ 需要时提取矢量证据 → 草稿校验与 id 重映射 →
 * 组装 IR（置信度由服务端给出）。持久化与事件由调用方（CircuitService）在写入队列里完成。
 * 同一份内容哈希的 IR 已存在时沿用 `parse` 的前置条件（§3.4）；内容变了就是一份新 IR，旧修正不迁移（§3.2）。
 */

export interface CircuitRecognitionPlan {
  record: StoredCircuit;
  /** 日志字段：只记 id、计数与耗时，不写电路内容（§9）。 */
  log: {
    kind: CircuitSourceKind;
    components: number;
    nets: number;
    blocks: number;
    diagnostics: number;
    lowConfidence: number;
    evidenceHits: number;
    orphanedCorrections: number;
    droppedCorrections: number;
    ms: number;
  };
}

export async function planCircuitRecognition(input: {
  location: CircuitLocation;
  params: CircuitSubmitRecognitionParams;
  stored?: StoredCircuit;
  source: CircuitSourcePort;
  vector: CircuitVectorPort;
}): Promise<CircuitRecognitionPlan> {
  const started = Date.now();
  const { location, params, stored } = input;
  // 扩展名先判后读：只有图片与 PDF 能识别，其它来源（如 .kicad_sch）报 unsupported_format。
  const declaredKind = detectRecognitionSourceKind(params.path);
  const bytes = await input.source.readBytes({
    workspacePath: location.workspacePath,
    relativePath: location.sourcePath,
  });
  const contentHash = input.source.hashBytes(bytes);
  const evidence = declaredKind === "pdf_vector" ? await input.vector.extract(bytes) : undefined;
  // 只有位图的 PDF 没有可提取的文本/线段，与截图同样按 image 处理（没有确定性证据）。
  const kind: CircuitSourceKind = evidence && isVectorEvidence(evidence) ? "pdf_vector" : "image";

  const sameContent = stored !== undefined && stored.ir.source.contentHash === contentHash;
  const revision = sameContent ? requireReparseRevision(stored, params.baseRevision) + 1 : 1;
  const irId = sameContent
    ? stored.ir.irId
    : circuitContentIrIdOf(location, contentHash, input.source.hashContent);

  const elements = buildRecognitionElements({
    draft: params.draft,
    ...(evidence ? { evidence } : {}),
  });
  const parsed = assembleCircuitIr({
    irId,
    revision,
    source: { kind, path: location.sourcePath, contentHash },
    stale: false,
    elements: {
      components: elements.components,
      nets: elements.nets,
      blocks: elements.blocks,
      diagnostics: elements.diagnostics,
    },
  });
  // 同一份内容 = 同一份 IR：按 id 重新套用已有修正（套不上的标 orphaned，§3.4）。
  // 内容变了（新 irId）时旧修正不迁移（§3.2）。
  const applied = sameContent
    ? reapplyCircuitCorrections(parsed, stored.corrections)
    : { ir: parsed, records: [] };
  const record: StoredCircuit = { irId, ir: applied.ir, corrections: applied.records };
  const plan: CircuitRecognitionPlan = {
    record,
    log: {
      kind,
      components: record.ir.components.length,
      nets: record.ir.nets.length,
      blocks: record.ir.blocks.length,
      diagnostics: record.ir.diagnostics.length,
      lowConfidence: countLowConfidenceElements(record.ir),
      evidenceHits: elements.evidenceHits,
      orphanedCorrections: record.corrections.filter((entry) => entry.orphaned).length,
      droppedCorrections: sameContent ? 0 : (stored?.corrections.length ?? 0),
      ms: Date.now() - started,
    },
  };
  log.info("circuit recognition", { irId, revision, ...plan.log });
  return plan;
}
