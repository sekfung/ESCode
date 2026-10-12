import { CIRCUIT_RESPONSE_MAX_BYTES } from "@escode/shared/circuit";
import { decodeCircuitCursor, encodeCircuitCursor } from "./circuitCursor.js";

/**
 * 分析结果（规则条目 / 固件差异）的分页（§5）：与 `circuit_query` 用同一口径——
 * 序列化后不超过响应上限的 75%，至少返回一条，超出的部分留给下一页；
 * 游标绑定查询签名（把签名一起编码进游标），拿错游标报 `invalid_ir`。
 */
const PAGE_BYTE_BUDGET = Math.floor(CIRCUIT_RESPONSE_MAX_BYTES * 0.75);

export function pageByByteBudget<T>(items: T[], offset: number): { page: T[]; hasMore: boolean } {
  const page: T[] = [];
  let bytes = 0;
  for (const item of items.slice(offset)) {
    const itemBytes = Buffer.byteLength(JSON.stringify(item), "utf8");
    if (page.length > 0 && bytes + itemBytes > PAGE_BYTE_BUDGET) break;
    page.push(item);
    bytes += itemBytes;
  }
  return { page, hasMore: offset + page.length < items.length };
}

/** 按游标取一页；还剩余条目时给出下一页游标。 */
export function paginateAnalysisResult<T>(
  items: T[],
  cursor: string | undefined,
  signature: string,
): { items: T[]; nextCursor?: string } {
  const offset = decodeCircuitCursor(cursor, signature);
  const { page, hasMore } = pageByByteBudget(items, offset);
  const nextCursor = hasMore ? encodeCircuitCursor(signature, offset + page.length) : undefined;
  return { items: page, ...(nextCursor ? { nextCursor } : {}) };
}
