import type { CircuitDraftReference } from "@/circuit/domain/circuitReferences.js";

/**
 * 电路引用 chip 的 renderer 端草稿（规格 §8）：
 * 面板「插入到输入框」写进这里，composer 显示 chip 并在发送时把引用块拼进 prompt；
 * chip 只属于输入草稿，**不会**自动发送——插入后草稿仍要用户自己发送。
 *
 * 与 conversationSelectionReference / pptxElementReference 同构：模块级 Map 按 workspace 隔离，
 * 用 window 事件通知已挂载的 composer，避免为一条草稿再引入一份全局 store。
 */

const ADD_EVENT = "escode:circuit-reference-draft-add";
const BLOCK_TITLE = "# Circuit references:";
const BLOCK_DIRECTIVE =
  "The user selected these circuit elements in the panel. Read them from the IR with the circuit tools (circuit_get / circuit_query) before answering; do not change the design unless the user asks.";
const referencesByWorkspace = new Map<string, readonly CircuitDraftReference[]>();

function parseItem(value: unknown): CircuitDraftReference | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const candidate = value as Partial<CircuitDraftReference>;
  if (
    typeof candidate.irId !== "string" ||
    candidate.irId.trim().length === 0 ||
    (candidate.kind !== "component" && candidate.kind !== "net") ||
    typeof candidate.name !== "string" ||
    typeof candidate.label !== "string" ||
    typeof candidate.detail !== "string"
  ) {
    return null;
  }
  return {
    irId: candidate.irId,
    kind: candidate.kind,
    name: candidate.name,
    label: candidate.label,
    detail: candidate.detail,
  };
}

export function getCircuitDraftReferences(workspaceKey: string): readonly CircuitDraftReference[] {
  return referencesByWorkspace.get(workspaceKey) ?? [];
}

export function setCircuitDraftReferences(
  workspaceKey: string,
  references: readonly CircuitDraftReference[],
): void {
  if (references.length > 0) referencesByWorkspace.set(workspaceKey, references);
  else referencesByWorkspace.delete(workspaceKey);
}

/** 重复插入同一个元素时保持一条（按 irId + 类型 + 名字去重，与草稿的可见 chip 对齐）。 */
export function appendCircuitDraftReference(
  current: readonly CircuitDraftReference[],
  reference: CircuitDraftReference,
): readonly CircuitDraftReference[] {
  const exists = current.some(
    (item) => item.irId === reference.irId && item.kind === reference.kind && item.name === reference.name,
  );
  return exists ? current : [...current, reference];
}

export function removeCircuitDraftReference(
  workspaceKey: string,
  reference: CircuitDraftReference,
): readonly CircuitDraftReference[] {
  const next = getCircuitDraftReferences(workspaceKey).filter(
    (item) => !(item.irId === reference.irId && item.kind === reference.kind && item.name === reference.name),
  );
  setCircuitDraftReferences(workspaceKey, next);
  return next;
}

export function clearCircuitDraftReferences(workspaceKey: string): void {
  setCircuitDraftReferences(workspaceKey, []);
}

export function dispatchCircuitDraftReference(
  workspaceKey: string,
  reference: CircuitDraftReference,
): void {
  const next = appendCircuitDraftReference(getCircuitDraftReferences(workspaceKey), reference);
  setCircuitDraftReferences(workspaceKey, next);
  if (typeof window !== "undefined")
    window.dispatchEvent(new CustomEvent(ADD_EVENT, { detail: { workspaceKey } }));
}

export function isCircuitDraftReferenceEvent(event: Event): event is CustomEvent<{ workspaceKey: string }> {
  return event.type === ADD_EVENT && event instanceof CustomEvent;
}

export function getCircuitDraftReferenceEventName(): string {
  return ADD_EVENT;
}

const BLOCK_PATTERN = new RegExp(
  `(?:^|\\n\\n)${BLOCK_TITLE.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&")}\\s*\\n\\n${BLOCK_DIRECTIVE.replace(
    /[.*+?^${}()|[\]\\]/gu,
    "\\$&",
  )}\\s*\\n\\n\`\`\`circuit-references\\n([\\s\\S]*?)\\n\`\`\`\\s*$`,
  "u",
);

export function buildPromptWithCircuitReferences(
  text: string,
  references: readonly CircuitDraftReference[],
): string {
  if (references.length === 0) return text;
  const body = references.map((reference) => ({
    irId: reference.irId,
    kind: reference.kind,
    name: reference.name,
    label: reference.label,
    detail: reference.detail,
  }));
  return `${text}\n\n${BLOCK_TITLE}\n\n${BLOCK_DIRECTIVE}\n\n\`\`\`circuit-references\n${JSON.stringify(body)}\n\`\`\``;
}

/** 历史 user row 解析：剥掉尾块并把 chip 还原出来（解析顺序与序列化顺序严格相反）。 */
export function parsePromptCircuitReferences(text: string): {
  visibleContent: string;
  circuitReferences: CircuitDraftReference[];
} {
  const match = text.match(BLOCK_PATTERN);
  if (!match) return { visibleContent: text, circuitReferences: [] };
  try {
    const parsed: unknown = JSON.parse(match[1] ?? "[]");
    if (!Array.isArray(parsed)) throw new Error("circuit reference block is not an array");
    const references: CircuitDraftReference[] = [];
    for (const value of parsed) {
      const reference = parseItem(value);
      if (!reference) throw new Error("invalid circuit reference");
      references.push(reference);
    }
    return { visibleContent: text.slice(0, match.index).trimEnd(), circuitReferences: references };
  } catch {
    return { visibleContent: text, circuitReferences: [] };
  }
}
