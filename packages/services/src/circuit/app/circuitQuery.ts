import {
  type CircuitComponent,
  type CircuitIR,
} from "@escode/shared/circuit";
import type { CircuitQueryParams, CircuitQueryResult } from "../contract.js";
import { CircuitServiceError } from "../domain/circuitErrors.js";
import { decodeCircuitCursor, encodeCircuitCursor } from "./circuitCursor.js";
import { pageByByteBudget } from "./analysisPagination.js";

/**
 * `circuit_query` 的服务端实现（§5）：按 ref / net / block 过滤，或从 componentId(+pin) 出发追踪连接。
 * 过滤条件省略时列出该类全部元素（面板的 BOM 与网络表就走这条路，读的仍然是 IR）；
 * 所有列表结果都按「序列化后不超过响应上限」分页，游标是不透明字符串（实现见 circuitCursor.ts，
 * 与 circuit_check / circuit_compare_firmware 共用）。
 */

export function runCircuitQuery(ir: CircuitIR, params: CircuitQueryParams): CircuitQueryResult {
  const signature = querySignature(ir.irId, params);
  const offset = decodeCircuitCursor(params.cursor, signature);
  switch (params.by) {
    case "ref": {
      const ref = params.ref;
      const items = (
        ref === undefined ? ir.components : ir.components.filter((component) => component.ref === ref)
      ).sort((a, b) => a.id.localeCompare(b.id));
      return paginate(items, offset, "ref", signature);
    }
    case "net": {
      const items = ir.nets
        .filter((net) => {
          if (params.netId !== undefined) return net.id === params.netId;
          if (params.netName !== undefined) return net.name === params.netName;
          return true;
        })
        .sort((a, b) => a.id.localeCompare(b.id));
      return paginate(items, offset, "net", signature);
    }
    case "block": {
      const items = ir.blocks
        .filter((block) => {
          if (params.blockId !== undefined) return block.id === params.blockId;
          if (params.blockName !== undefined) return block.name === params.blockName;
          return true;
        })
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
  return encodeCircuitCursor(signature, offset);
}

/** 按字节预算分页：至少给一条，超出预算的条目留到下一页。 */
function paginate<T>(
  items: T[],
  offset: number,
  by: "ref" | "net" | "block" | "trace",
  signature: string,
): CircuitQueryResult {
  const { page, hasMore } = pageByByteBudget(items, offset);
  const nextCursor = hasMore ? encodeCircuitCursor(signature, offset + page.length) : undefined;
  return { by, items: page, ...(nextCursor ? { nextCursor } : {}) } as CircuitQueryResult;
}
