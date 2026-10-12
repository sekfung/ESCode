import {
  CIRCUIT_AGENT_TOOL_OPS,
  circuitToolResponseSchemas,
  escodeCircuitMethods,
  escodeCircuitMethodParamsSchemas,
  type CircuitAgentToolOp,
} from "@escode/shared/circuit";
import type { CircuitAgentBridge } from "./circuitAgentBridge.js";

type RequestId = string | number;

interface Responder {
  respond(id: RequestId, result: unknown): Promise<void>;
  respondError(
    id: RequestId,
    error: { code: number; message: string; data?: unknown },
  ): Promise<void>;
}

const OP_BY_METHOD = new Map<string, CircuitAgentToolOp>(
  CIRCUIT_AGENT_TOOL_OPS.map((op) => [escodeCircuitMethods[op], op]),
);

/**
 * Agent 运行时发来的 interaction/circuit* 反向请求。返回 false 表示不是电路方法，交给后续分支。
 * 与 serial 的差别：工具失败是响应信封（`{ ok: false, error }`）而不是 JSON-RPC error，
 * 所以这里只校验请求参数与响应结构，业务错误码在信封里原样带回（见 circuitAgentBridge）。
 */
export function routeCircuitAgentRequest(options: {
  request: { id: RequestId; method: string; params?: unknown };
  bridge: CircuitAgentBridge | undefined;
  responder: Responder;
}): boolean {
  const { request, bridge, responder } = options;
  const reply = (promise: Promise<void>) => {
    promise.catch(() => {
      // 运行时连接已断开时应答会失败；请求方已不在，无需再处理。
    });
  };

  const op = OP_BY_METHOD.get(request.method);
  if (!op) return false;
  const parsed = escodeCircuitMethodParamsSchemas[op].safeParse(request.params);
  if (!parsed.success) {
    reply(
      responder.respondError(request.id, {
        code: -32602,
        message: `Invalid ${request.method} params`,
        data: parsed.error.flatten(),
      }),
    );
    return true;
  }
  if (!bridge) {
    // 没有桥接（Host 未装配电路工具）时按 unavailable 工具失败应答，模型能看懂原因。
    reply(
      responder.respond(request.id, {
        ok: false,
        error: { code: "unavailable", message: "Circuit tools are not available here" },
      }),
    );
    return true;
  }
  reply(
    bridge
      .handle(op, parsed.data as never)
      .then((result) => {
        // 响应用工具总表校验；实现域内散出非法结构属于缺陷，按内部错误报告而不是静默透传。
        const response = circuitToolResponseSchemas[op].safeParse(result);
        if (!response.success)
          return responder.respondError(request.id, {
            code: -32603,
            message: `Invalid ${request.method} response`,
            data: { code: "io_error" },
          });
        return responder.respond(request.id, response.data);
      })
      .catch((error: unknown) =>
        responder.respondError(request.id, {
          code: -32603,
          message: error instanceof Error ? error.message : String(error),
          data: { code: "io_error" },
        }),
      ),
  );
  return true;
}
