import {
  CIRCUIT_RESPONSE_MAX_BYTES,
  type CircuitComponent,
  type CircuitIR,
} from "@escode/shared/circuit";
import type { CircuitQueryParams, CircuitQueryResult } from "../contract.js";
import { CircuitServiceError } from "../domain/circuitErrors.js";

/**
 * `circuit_query` 的服务端实现（§5）：按 ref / net / block 过滤，或从 componentId(+pin) 出发追踪连接。
 * 所有列表结果都按「序列化后不超过响应上限」分页，游标是不透明字符串（这里用 base64url JSON 实现）。
 */

const PAGE_BYTE_BUDGET = Math.floor(CIRCUIT_RESPONSE_MAX_BYTES * 0.75);

export function runCircuitQuery(ir: CircuitIR, params: CircuitQueryParams): CircuitQueryResult {
  const signature = querySignature(ir.irId, params);
  const offset = decodeCursor(params.cursor, signature);
  switch (params.by) {
    case "ref": {
      const ref = params.ref;
      if (ref === undefined) throw new CircuitServiceError("invalid_ir", "按 ref 查询需要 ref");
      const items = ir.components
        .filter((component) => component.ref === ref)
        .sort((a, b) => a.id.localeCompare(b.id));
      return paginate(items, offset, "ref", signature);
    }
    case "net": {
      const items = ir.nets
        .filter((net) =>
          params.netId !== undefined ? net.id === params.netId : net.name === params.netName,
        )
        .sort((a, b) => a.id.localeCompare(b.id));
      return paginate(items, offset, "net", signature);
    }
    case "block": {
      const items = ir.blocks
        .filter((block) =>
          params.blockId !== undefined ? block.id === params.blockId : block.name === params.blockName,
        )
        .sort((a, b) => a.id.localeCompare(b.id));
      return paginate(items, offset, "block", signature);
    }
    case "trace": {
      const componentId = params.componentId;
      if (componentId === undefined)
        throw new CircuitServiceError("invalid_ir", "追踪连接需要 componentId");
      const component = ir.components.find((candidate) => candidate.id === componentId);
      if (!component) throw new CircuitServiceError("not_found", `器件 ${componentId} 不在 IR 中`);
      const pin = params.pin;
      if (pin !== undefined && !component.pins.some((candidate) => candidate.number === pin))
        throw new CircuitServiceError("not_found", `器件 ${component.ref} 没有引脚 ${pin}`);
      const netIds = [
        ...new Set(
          (pin !== undefined
            ? [component.pins.find((candidate) => candidate.number === pin)?.netId]
            : component.pins.map((candidate) => candidate.netId)
          ).filter((netId): netId is string => netId !== undefined),
        ),
      ].sort();
      const items = netIds.map((netId) => {
        const net = ir.nets.find((candidate) => candidate.id === netId)!;
        const connected = net.pinRefs
          .map((pinRef) => ir.components.find((candidate) => candidate.id === pinRef.componentId))
          .filter((candidate): candidate is CircuitComponent => candidate !== undefined)
          .filter(
            (candidate, index, list) =>
              list.findIndex((other) => other.id === candidate.id) === index,
          )
          .sort((a, b) => a.id.localeCompare(b.id));
        return {
          componentId: component.id,
          ...(pin !== undefined ? { pin } : {}),
          netIds: [net.id],
          nets: [net],
          components: connected,
        };
      });
      return paginate(items, offset, "trace", signature);
    }
  }
}

function querySignature(irId: string, params: CircuitQueryParams): string {
  return JSON.stringify([
    irId,
    params.by,
    params.ref ?? null,
    params.netId ?? null,
    params.netName ?? null,
    params.blockId ?? null,
    params.blockName ?? null,
    params.componentId ?? null,
    params.pin ?? null,
  ]);
}

export function encodeCursor(signature: string, offset: number): string {
  return Buffer.from(JSON.stringify({ q: hash(signature), o: offset }), "utf8").toString("base64url");
}

function decodeCursor(cursor: string | undefined, signature: string): number {
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

/** 按字节预算分页：至少给一条，超出预算的条目留到下一页。 */
function paginate<T>(
  items: T[],
  offset: number,
  by: "ref" | "net" | "block" | "trace",
  signature: string,
): CircuitQueryResult {
  const page: T[] = [];
  let bytes = 0;
  for (const item of items.slice(offset)) {
    const itemBytes = Buffer.byteLength(JSON.stringify(item), "utf8");
    if (page.length > 0 && bytes + itemBytes > PAGE_BYTE_BUDGET) break;
    page.push(item);
    bytes += itemBytes;
  }
  const consumed = offset + page.length;
  const nextCursor = consumed < items.length ? encodeCursor(signature, consumed) : undefined;
  return { by, items: page, ...(nextCursor ? { nextCursor } : {}) } as CircuitQueryResult;
}
