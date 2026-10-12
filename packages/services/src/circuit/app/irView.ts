import {
  CIRCUIT_RESPONSE_MAX_BYTES,
  type CircuitDiagnostic,
  type CircuitIR,
} from "@escode/shared/circuit";
import type { CircuitIrView } from "../contract.js";
import { toCircuitIrSummary } from "@escode/shared/circuit";

/** 单次响应给 diagnostics / items 留的字节预算（§5 的 64 KiB 上限）。 */
export const CIRCUIT_VIEW_BYTE_BUDGET = Math.floor(CIRCUIT_RESPONSE_MAX_BYTES * 0.75);

/** 工具与面板读取的 IR 视图：摘要 + 校验问题明细（超出上限时截断并置 truncated）。 */
export function circuitIrViewOf(ir: CircuitIR): CircuitIrView {
  const summary = toCircuitIrSummary(ir);
  const diagnostics: CircuitDiagnostic[] = [];
  let bytes = 0;
  for (const diagnostic of ir.diagnostics) {
    const size = Buffer.byteLength(JSON.stringify(diagnostic), "utf8");
    if (diagnostics.length > 0 && bytes + size > CIRCUIT_VIEW_BYTE_BUDGET) break;
    diagnostics.push(diagnostic);
    bytes += size;
  }
  return { summary, diagnostics, truncated: diagnostics.length < ir.diagnostics.length };
}
