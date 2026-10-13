/**
 * EasyEDA（嘉立创EDA）V3 源码日志的解码（格式见 easyeda-api skill 的 `format/index.md`）。
 *
 * 文件按行存储：每行是 `{outer}||{inner}|` 两个 JSON。`DOCHEAD` 划分文档——遇到它就表示
 * 后面的数据属于该文档，直到下一个 `DOCHEAD`。同一 `(type, id)` 的多条记录按最终一致性收敛：
 * 保留 `ticket` 最大的，`ticket` 相同保留 `client` 较小的；`inner` 为 `""` 表示删除该条数据
 * （删除同样要按 ticket 比较，落后于现有记录的删除不生效）。
 * 纯函数、零 IO；无法解析的行直接跳过——格式不符的地方不猜。
 */

export interface EasyedaRecord {
  type: string;
  /** 同一 type 内唯一；单例数据（如 META）省略。 */
  id?: string;
  ticket?: number;
  /** 所属文档的 client，用于 ticket 相同时的仲裁。 */
  client?: string;
  /** 内层载荷；`""` 表示这条数据被删除。 */
  payload: Record<string, unknown>;
}

export interface EasyedaDocument {
  docType: string;
  uuid: string;
  client?: string;
  /** 按最终一致性收敛后的记录，保持首次出现的顺序。 */
  records: EasyedaRecord[];
}

/** 文档头载荷（`docType` / `uuid` / `client`）。 */
interface EasyedaDocHead {
  docType: string;
  uuid: string;
  client?: string;
}

function splitRecord(line: string): { outer: Record<string, unknown>; innerRaw: string } | null {
  const separator = line.indexOf("||");
  if (separator <= 0) return null;
  // 记录以 `|` 结尾；去掉它再解析内层。
  const innerRaw = line.slice(separator + 2).replace(/\|\s*$/u, "");
  try {
    const outer = JSON.parse(line.slice(0, separator)) as unknown;
    if (outer === null || typeof outer !== "object" || Array.isArray(outer)) return null;
    return { outer: outer as Record<string, unknown>, innerRaw };
  } catch {
    return null;
  }
}

function parseDocHead(innerRaw: string, fallbackClient: string | undefined): EasyedaDocHead | null {
  try {
    const inner = JSON.parse(innerRaw) as unknown;
    if (inner === null || typeof inner !== "object") return null;
    const head = inner as { docType?: unknown; uuid?: unknown; client?: unknown };
    if (typeof head.docType !== "string" || typeof head.uuid !== "string") return null;
    const client =
      typeof head.client === "string"
        ? head.client
        : typeof head.client === "number"
          ? String(head.client)
          : fallbackClient;
    return { docType: head.docType, uuid: head.uuid, ...(client === undefined ? {} : { client }) };
  } catch {
    return null;
  }
}

function parsePayload(innerRaw: string): Record<string, unknown> | null {
  try {
    const inner = JSON.parse(innerRaw) as unknown;
    if (inner === null || typeof inner !== "object" || Array.isArray(inner)) return null;
    return inner as Record<string, unknown>;
  } catch {
    return null;
  }
}

function ticketOf(record: EasyedaRecord): number {
  return typeof record.ticket === "number" ? record.ticket : 0;
}

/** 新记录是否胜出：ticket 大者胜；ticket 相同比 client，client 小者胜（格式的最终一致性口径）。 */
function supersedes(candidate: EasyedaRecord, current: EasyedaRecord): boolean {
  const candidateTicket = ticketOf(candidate);
  const currentTicket = ticketOf(current);
  if (candidateTicket !== currentTicket) return candidateTicket > currentTicket;
  return (candidate.client ?? "") < (current.client ?? "");
}

/**
 * 解码整份日志为若干文档。`docType` 为 `DELETE_DOC` 且 `isDelete` 为真的文档整份丢弃
 * （删除标记只表示文档被删，日志里的数据仍保留）。
 */
export function decodeEasyedaLog(text: string): EasyedaDocument[] {
  const documents: EasyedaDocument[] = [];
  let head: EasyedaDocHead | null = null;
  let merged: Map<string, EasyedaRecord> | null = null;

  const flush = (): void => {
    if (head === null || merged === null) return;
    documents.push({
      docType: head.docType,
      uuid: head.uuid,
      ...(head.client === undefined ? {} : { client: head.client }),
      records: [...merged.values()],
    });
  };

  for (const rawLine of text.split(/\r?\n/u)) {
    const line = rawLine.trim();
    if (line.length === 0) continue;
    const split = splitRecord(line);
    if (split === null) continue;
    const { outer, innerRaw } = split;
    const type = outer["type"];
    if (typeof type !== "string") continue;

    if (type === "DOCHEAD") {
      flush();
      head = parseDocHead(innerRaw, head?.client);
      merged = new Map();
      continue;
    }
    if (head === null || merged === null) continue; // DOCHEAD 之前的数据不属于任何文档。

    // 内层为空串（`||"|` 或 `|||`）表示删除；其余必须是对象。
    const trimmedInner = innerRaw.trim();
    const deleted = trimmedInner.length === 0 || trimmedInner === '""';
    const payload = deleted ? null : parsePayload(innerRaw);
    if (!deleted && payload === null) continue;

    const rawId = outer["id"];
    const id = typeof rawId === "string" ? rawId : undefined;
    const ticket = typeof outer["ticket"] === "number" ? outer["ticket"] : undefined;
    if (id === undefined && type === "DELETE_DOC") {
      if ((payload as Record<string, unknown> | null)?.["isDelete"] === true) {
        head = null;
        merged = null;
      }
      continue;
    }

    const key = `${type}\u0000${id ?? ""}`;
    const candidate: EasyedaRecord = {
      type,
      ...(id === undefined ? {} : { id }),
      ...(ticket === undefined ? {} : { ticket }),
      ...(head.client === undefined ? {} : { client: head.client }),
      payload: {},
    };
    const current = merged.get(key);
    if (current !== undefined && !supersedes(candidate, current)) continue;
    if (deleted) merged.delete(key);
    else merged.set(key, { ...candidate, payload: payload! });
  }
  flush();
  return documents;
}
