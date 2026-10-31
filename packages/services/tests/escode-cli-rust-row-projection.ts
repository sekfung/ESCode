/**
 * 会话行投影（差分/真实数据演练共用）。
 *
 * 目标：只归一化随机值与运行环境事实，其余字段全部参与比较，避免 L2 投影把语义字段抹平。
 * - id 按首次出现映射为稳定占位，保留同一性与跨行关联；
 * - 时间戳、时长只保留存在性；
 * - createdAtSeq 按值排名保留顺序关系（投递层实现差异已登记，清账后可收紧）；
 * - userInput / assistantText 的 entityId 取值方式不同（docs/specs/rust-row-projection.md），只比存在性。
 */

type Row = Record<string, any>;

const ID_KEYS = new Set([
  "turnId",
  "productTurnId",
  "entityId",
  "sourceCommandId",
  "rootSourceCommandId",
  "clientId",
  "assistantResponseId",
  "toolCallId",
  "parentToolCallId",
  "childSessionId",
  "parentSessionId",
  "hookInvocationId",
  "artifactVersionId",
]);

const OPAQUE_ENTITY_KINDS = new Set(["userInput", "assistantText"]);

export function projectRows(rows: Row[], roots: string[]): Row[] {
  const idMap = new Map<string, string>();
  const seqRanks = new Map<number, number>();
  const replacements = [...new Set(roots)]
    .filter(Boolean)
    .sort((a, b) => b.length - a.length)
    .flatMap(
      (root) =>
        [
          [root, "<root>"],
          [JSON.stringify(root).slice(1, -1), "<root>"],
        ] as const,
    );

  const projectId = (value: string) => {
    if (!idMap.has(value)) idMap.set(value, `<id:${idMap.size + 1}>`);
    return idMap.get(value)!;
  };

  const project = (value: unknown, key: string | undefined, kind: string): unknown => {
    if (Array.isArray(value)) return value.map((item) => project(item, key, kind));
    if (value && typeof value === "object") {
      return Object.fromEntries(
        Object.entries(value)
          .filter(([, item]) => item !== undefined)
          .map(([childKey, item]) => [childKey, project(item, childKey, kind)]),
      );
    }
    if (typeof value === "string") {
      let rooted = value;
      for (const [from, to] of replacements) rooted = rooted.replaceAll(from, to);
      rooted = rooted.replaceAll("\\", "/");
      if (key && ID_KEYS.has(key)) {
        if (key === "entityId" && OPAQUE_ENTITY_KINDS.has(kind)) return "<entityId>";
        return projectId(rooted);
      }
      return rooted;
    }
    if (typeof value === "number") {
      if (key === "createdAtSeq") {
        if (!seqRanks.has(value)) seqRanks.set(value, seqRanks.size + 1);
        return `<seq:${seqRanks.get(value)}>`;
      }
      if (key === "createdAt" || (key && /(?:At|Time)$/.test(key))) return "<timestamp>";
      if (key && /(?:Ms|Duration)$/.test(key)) return "<duration>";
    }
    return value;
  };

  return rows.map((row) =>
    Object.fromEntries(
      Object.entries(row)
        .filter(([, item]) => item !== undefined)
        .map(([key, item]) => [key, project(item, key, row.kind)]),
    ),
  );
}
