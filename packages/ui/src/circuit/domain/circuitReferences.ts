import { circuitReferenceSchema, type CircuitNet, type CircuitReference } from "@escode/shared/circuit";
import {
  extractAssistantDirectives,
  findAssistantDirectivePrefixStart,
  findMarkdownCodeRanges,
  findUnclosedAssistantDirectiveStart,
} from "@/lib/assistantDirectiveParser.js";

/**
 * `::circuit{"irId":"…","title":"…"}` 引用卡片的解析（规格 §8）。
 *
 * 解析规则完全沿用 Gen UI 引用（§8「解析规则沿用 Gen UI 引用」）：
 * 代码块（围栏 / 行内 / <pre>）内不识别，消息完成后才渲染，流式半截 directive 先隐藏；
 * 载荷只认 `circuitReferenceSchema`（shared/circuit/analysisTools.ts）。
 * agent 侧的生成时机见 circuit skill：`circuit_parse` / `circuit_get` 之后由 Agent 自己写进回复。
 */

export const CIRCUIT_DIRECTIVE_NAME = "circuit";

export type CircuitReferencePart =
  | { kind: "text"; text: string }
  | { kind: "circuit"; reference: CircuitReference; offset: number }
  | { kind: "pending" | "invalid" };

interface CircuitReferenceCandidate {
  start: number;
  end: number;
  decode(): unknown;
}

// JSON 参数前缀：允许字符串与数字（componentCount / netCount 是数字），
// 与 Gen UI 的同名常量分工一致——只隐藏仍然是「参数前缀」的半截内容。
const jsonString = String.raw`"(?:[^"\\\r\n]|\\.)*"`;
const jsonParameterPrefix = new RegExp(
  String.raw`^\s*(?:${jsonString}\s*:\s*(?:${jsonString}|-?\d+(?:\.\d+)?)\s*,\s*)*(?:${jsonString}(?:\s*:\s*(?:${jsonString}|-?\d+(?:\.\d+)?)?)?)?\s*$`,
);

export function projectCircuitReferences(text: string, completed: boolean): CircuitReferencePart[] {
  const ranges = findMarkdownCodeRanges(text);
  const parts: CircuitReferencePart[] = [];
  const candidates: CircuitReferenceCandidate[] = extractAssistantDirectives(
    text,
    CIRCUIT_DIRECTIVE_NAME,
  ).map((directive) => ({
    ...directive,
    // key=value 形态与 JSON 形态都接受：key=value 只能给字符串字段（irId/title），
    // JSON 形态才能带 componentCount / netCount（降级卡片的器件数与网络数）。
    decode: () =>
      directive.parameters ?? JSON.parse(directive.raw.slice(directive.raw.indexOf("{"))),
  }));
  const unclosed = findUnclosedAssistantDirectiveStart(text, CIRCUIT_DIRECTIVE_NAME, ranges, {
    isParameterPrefix: (source) => jsonParameterPrefix.test(source),
  });
  if (unclosed !== null) candidates.push({ start: unclosed, end: text.length, decode: () => null });
  let cursor = 0;
  for (const candidate of candidates.sort((left, right) => left.start - right.start)) {
    const { start, end } = candidate;
    if (ranges.some(([from, to]) => start >= from && start < to) || start < cursor) continue;
    if (start > cursor) parts.push({ kind: "text", text: text.slice(cursor, start) });
    cursor = end;
    if (!completed) {
      parts.push({ kind: "pending" });
      continue;
    }
    try {
      parts.push({
        kind: "circuit",
        reference: circuitReferenceSchema.parse(candidate.decode()),
        offset: start,
      });
    } catch {
      parts.push({ kind: "invalid" });
    }
  }
  let tailEnd = text.length;
  if (!completed) {
    const prefix = findAssistantDirectivePrefixStart(text, [CIRCUIT_DIRECTIVE_NAME], ranges);
    if (prefix !== null && prefix >= cursor) tailEnd = prefix;
  }
  const tail = text.slice(cursor, tailEnd);
  if (tail) parts.push({ kind: "text", text: tail });
  return parts;
}

/** 插入输入框的引用 chip：`U3` / `NET:I2C1_SDA`，都带上 irId（§8）。 */
export interface CircuitDraftReference {
  irId: string;
  kind: "component" | "net";
  /** 器件位号或网络名；网络没有名字时退化成网络 id。 */
  name: string;
  /** chip 展示文本：`U3` / `NET:I2C1_SDA`。 */
  label: string;
  /** 写进 prompt 尾块的一行描述，模型据此知道指向哪个 IR 元素。 */
  detail: string;
}

function normalizeChipName(value: string): string {
  return value.trim().replace(/\s+/gu, "_");
}

export function circuitComponentDraftReference(
  irId: string,
  component: { id: string; ref: string },
): CircuitDraftReference {
  const name = normalizeChipName(component.ref);
  return {
    irId,
    kind: "component",
    name,
    label: name,
    detail: `${name} (component ${component.id})`,
  };
}

export function circuitNetDraftReference(irId: string, net: CircuitNet): CircuitDraftReference {
  const name = normalizeChipName(net.name ?? net.id);
  return {
    irId,
    kind: "net",
    name,
    label: `NET:${name}`,
    detail: `NET:${name} (net ${net.id})`,
  };
}
