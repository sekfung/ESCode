import type { StoredCircuit } from "./ports.js";
import { CircuitServiceError } from "../domain/circuitErrors.js";

/** 修正记录的维护（§3.3 的 revision 前置条件、§8 的 orphaned 清理），IO 由服务负责。 */

/** 写入前置条件：baseRevision 必须等于当前 revision，否则调用方应重读后重试。 */
export function requireCircuitRevision(record: StoredCircuit, baseRevision: number): void {
  if (baseRevision === record.ir.revision) return;
  throw new CircuitServiceError(
    "stale_revision",
    `当前 revision 为 ${record.ir.revision}，收到 baseRevision ${baseRevision}；请重读后再写入`,
  );
}

/** 删除一条 orphaned 修正记录；未失效的记录不允许删除（会与 IR 状态不一致）。 */
export function dropOrphanedCorrection(record: StoredCircuit, index: number): StoredCircuit {
  const entry = record.corrections[index];
  if (!entry) throw new CircuitServiceError("invalid_ir", `修正记录下标 ${index} 不存在`);
  if (!entry.orphaned)
    throw new CircuitServiceError("invalid_ir", "只能删除已失效（orphaned）的修正记录");
  return {
    ...record,
    corrections: record.corrections.filter((_, candidate) => candidate !== index),
  };
}
