import type { CircuitDiagnosticCode } from "@escode/shared/circuit";

/**
 * 识别结果的置信度规则（docs/circuit/README.md §2 规则 2/3、§4）：
 * 置信度只能由服务端给出——基准 0.9，按校验结果扣分，夹在 [0.05, 0.9]，**永不等于 1**
 * （1 只留给确定性解析：KiCad / 网表 / BOM / 矢量 PDF 里可直接提取的文本与线段）。
 * 纯函数、零 IO；判定口径见 packages/services/src/circuit/CONTRACT.md。
 */

/** 基准分：模型识别出的元素在没有任何校验问题时的置信度。 */
export const CIRCUIT_RECOGNITION_BASE_CONFIDENCE = 0.9;
/** 下限：扣分再多也不低于它，保证低置信度元素仍然出现在 IR 里（§2 规则 4 要能看见）。 */
export const CIRCUIT_RECOGNITION_MIN_CONFIDENCE = 0.05;
/** 上限：识别元素的置信度不允许达到确定性解析的 1（§2 规则 2）。 */
export const CIRCUIT_RECOGNITION_MAX_CONFIDENCE = 0.9;

/**
 * 扣分表（§4 的一致性校验项 → 诊断码）。同一元素命中同一类只扣一次，不同类累加：
 * - 单引脚网络 −0.1；
 * - 引脚数与封装不一致 −0.2；
 * - 位号重复 −0.3；
 * - 引用了不存在的器件/网络 −0.3（pinRefs 指向不存在的器件，或 netId 指向不存在的网络）。
 * `dangling-net`（零引脚网络）不在扣分表里：规格只列了上面四类，它仍然写入 diagnostics。
 */
export const CIRCUIT_RECOGNITION_PENALTIES: Partial<Record<CircuitDiagnosticCode, number>> = {
  "single-pin-net": 0.1,
  "pin-count-mismatch": 0.2,
  "duplicate-ref": 0.3,
  "unknown-component-reference": 0.3,
  "unknown-net-reference": 0.3,
};

export interface RecognitionConfidence {
  /** 夹在 [0.05, 0.9] 的置信度（保留 3 位小数，避免浮点尾数进入 IR）。 */
  confidence: number;
  /** 实际扣掉的分数。 */
  deducted: number;
  /** 生效的扣分类别（去重后按字典序，便于日志与断言）。 */
  categories: CircuitDiagnosticCode[];
}

/**
 * 按涉及该元素的诊断类别计算置信度：`0.9 − Σ扣分`，夹在 [0.05, 0.9]。
 * 传入的诊断码可以是重复的（同一个元素出现在多条相同诊断里），这里按类别去重。
 */
export function recognitionConfidence(
  codes: Iterable<CircuitDiagnosticCode>,
): RecognitionConfidence {
  const categories = [...new Set(codes)]
    .filter((code) => CIRCUIT_RECOGNITION_PENALTIES[code] !== undefined)
    .sort();
  const deducted = categories.reduce(
    (total, code) => total + (CIRCUIT_RECOGNITION_PENALTIES[code] ?? 0),
    0,
  );
  const raw = CIRCUIT_RECOGNITION_BASE_CONFIDENCE - deducted;
  const confidence = Math.min(
    CIRCUIT_RECOGNITION_MAX_CONFIDENCE,
    Math.max(CIRCUIT_RECOGNITION_MIN_CONFIDENCE, Math.round(raw * 1000) / 1000),
  );
  return { confidence, deducted: Math.round(deducted * 1000) / 1000, categories };
}
