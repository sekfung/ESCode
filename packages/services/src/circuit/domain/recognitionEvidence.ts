import type { CircuitRegion } from "@escode/shared/circuit";

/**
 * 矢量 PDF 的确定性证据（docs/circuit/README.md §2 规则 3、§5 circuit_submit_recognition）：
 * 服务端自己从 PDF 里提取文本项与线段，草稿元素在 region 附近命中文本证据时
 * `origin.by = "parser"`、`confidence = 1`。这里只定义证据结构与匹配口径（纯函数、零 IO）；
 * 提取本身在 adapters 里用 pdfjs 完成。
 *
 * 坐标口径：提取结果与草稿的 `region` 都用**渲染后左上角为原点**的源文件坐标
 * （图片=像素，PDF=点，1:1 缩放），与面板在原图上叠识别框用的坐标一致。
 */

export interface CircuitVectorText {
  /** 1 起始的页码（单页原理图恒为 1）。 */
  page: number;
  /** 已去掉首尾空白的文本内容。 */
  text: string;
  x: number;
  y: number;
}

export interface CircuitVectorSegment {
  page: number;
  x1: number;
  y1: number;
  x2: number;
  y2: number;
}

export interface CircuitVectorEvidence {
  /** 页数；region.page 超出页数时该元素拿不到证据。 */
  pageCount: number;
  texts: CircuitVectorText[];
  segments: CircuitVectorSegment[];
}

/** 命中判定用的最少容差（点/像素）：region 很小时也要能包住文字自身的尺寸。 */
export const CIRCUIT_EVIDENCE_MIN_TOLERANCE = 2;
/** 容差与 region 尺寸的比例：模型给的框比文字略小/略偏时不丢证据。 */
export const CIRCUIT_EVIDENCE_TOLERANCE_RATIO = 0.25;

/**
 * 这份提取结果算不算「矢量 PDF」：至少有一个文本项或线段。
 * 只有位图的 PDF（扫描件）在 pdfjs 下两者都为空，按 image 处理（没有确定性证据）。
 */
export function isVectorEvidence(evidence: CircuitVectorEvidence): boolean {
  return evidence.texts.length > 0 || evidence.segments.length > 0;
}

/** 文本证据比较用同一化：去首尾空白 + 不区分大小写（模型可能写小写位号）。 */
function normalizeEvidenceText(text: string): string {
  return text.trim().toLowerCase();
}

/**
 * region 附近是否存在等于候选之一（如器件的位号、网络的名称）的文本证据。
 * 判定：页匹配（region.page 缺省时任意页）+ 文本点落在按 region 尺寸放大的矩形内。
 * 只做精确串匹配，不做模糊匹配——证据用来把置信度提到 1，误命中比漏命中更糟。
 */
export function evidenceHasText(
  evidence: CircuitVectorEvidence | undefined,
  region: CircuitRegion | undefined,
  candidates: readonly string[],
): boolean {
  if (!evidence || !region) return false;
  const wanted = new Set(
    candidates.map(normalizeEvidenceText).filter((candidate) => candidate.length > 0),
  );
  if (wanted.size === 0) return false;
  const tolerance = Math.max(
    CIRCUIT_EVIDENCE_MIN_TOLERANCE,
    CIRCUIT_EVIDENCE_TOLERANCE_RATIO * Math.max(region.w, region.h),
  );
  return evidence.texts.some(
    (text) =>
      (region.page === undefined || text.page === region.page) &&
      text.x >= region.x - tolerance &&
      text.x <= region.x + region.w + tolerance &&
      text.y >= region.y - tolerance &&
      text.y <= region.y + region.h + tolerance &&
      wanted.has(normalizeEvidenceText(text.text)),
  );
}
