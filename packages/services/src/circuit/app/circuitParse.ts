import type { CircuitParseParams } from "../contract.js";
import { reapplyCircuitCorrections } from "../domain/circuitCorrections.js";
import { assembleCircuitIr } from "../domain/irBuild.js";
import { circuitIrIdOf, type CircuitLocation } from "./circuitLocation.js";
import type { CircuitSourcePort, StoredCircuit } from "./ports.js";
import { detectCircuitSourceKind, extractCircuitSource } from "./sourceExtraction.js";

/**
 * `circuit_parse` 的编排（§3.1、§3.4）：读源文件 → 确定性提取（含子图纸与渲染几何）→ 组装 IR →
 * 按 id 重新套用已有修正（套不上的标 orphaned）。持久化与事件由调用方（CircuitService）在写入队列里完成。
 * 只算「下一份记录」，不碰 store，也不发事件。
 */

export interface CircuitParsePlan {
  record: StoredCircuit;
  /** 这次真正读到的源文件（含子图纸），解析成功后逐个监听（§3.4）。 */
  sourceFiles: string[];
}

export async function planCircuitParse(input: {
  location: CircuitLocation;
  params: CircuitParseParams;
  stored?: StoredCircuit;
  source: CircuitSourcePort;
}): Promise<CircuitParsePlan> {
  const { location, source, stored } = input;
  const kind = detectCircuitSourceKind(location.sourcePath);
  const text = await source.readText({
    workspacePath: location.workspacePath,
    relativePath: location.sourcePath,
  });
  const contentHash = source.hashContent(text);
  const irId = stored?.ir.irId ?? circuitIrIdOf(location, source.hashContent);
  const revision = (stored?.ir.revision ?? 0) + 1;
  const sourceFiles = new Set<string>([location.sourcePath]);
  const { elements, layout } = await extractCircuitSource({
    kind,
    text,
    workspacePath: location.workspacePath,
    sourcePath: location.sourcePath,
    irId,
    revision,
    source,
    sourceFiles,
  });
  const parsed = assembleCircuitIr({
    irId,
    revision,
    source: { kind, path: location.sourcePath, contentHash },
    stale: false,
    elements,
  });
  const reapplied = reapplyCircuitCorrections(parsed, stored?.corrections ?? []);
  return {
    record: {
      irId,
      ir: reapplied.ir,
      ...(layout ? { layout: { ...layout, revision } } : {}),
      corrections: reapplied.records,
    },
    sourceFiles: [...sourceFiles],
  };
}
