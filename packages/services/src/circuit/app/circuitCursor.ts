import { CircuitServiceError } from "../domain/circuitErrors.js";

/**
 * 分析类结果的分页游标（§5「单次响应有字节上限，超出部分通过 cursor 分页获取」）。
 * 游标是不透明字符串：base64url(JSON{q,o})，`q` 绑定查询签名，`o` 是下一条的起始下标；
 * 签名不匹配时拒绝，避免拿 A 查询的游标翻 B 查询的页。实现与 `circuit_query` 共用一份。
 */

export function encodeCircuitCursor(signature: string, offset: number): string {
  return Buffer.from(JSON.stringify({ q: hash(signature), o: offset }), "utf8").toString("base64url");
}

export function decodeCircuitCursor(cursor: string | undefined, signature: string): number {
  if (cursor === undefined) return 0;
  let parsed: { q?: string; o?: number };
  try {
    parsed = JSON.parse(Buffer.from(cursor, "base64url").toString("utf8")) as {
      q?: string;
      o?: number;
    };
  } catch {
    throw new CircuitServiceError("invalid_ir", "cursor 无法解析");
  }
  if (parsed.q !== hash(signature) || typeof parsed.o !== "number" || parsed.o < 0)
    throw new CircuitServiceError("invalid_ir", "cursor 与当前查询不匹配");
  return parsed.o;
}

function hash(value: string): string {
  let result = 2166136261;
  for (const char of value) {
    result ^= char.codePointAt(0)!;
    result = Math.imul(result, 16777619);
  }
  return (result >>> 0).toString(16);
}
