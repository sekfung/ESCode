import {
  SERIAL_TOOL_OPS,
  zcodeSerialCancelMethod,
  zcodeSerialCancelParamsSchema,
  zcodeSerialMethodParamsSchemas,
  zcodeSerialMethods,
  type SerialToolOp,
} from "@zcode/shared/serial";
import { SerialError } from "./serial.js";
import type { SerialAgentBridge } from "./serialAgentBridge.js";

/** 串口工具的业务失败（busy、notOpen 等）；error.data.code 为 SerialToolErrorCode，broker 原样转成工具失败。 */
export const SERIAL_TOOL_ERROR_RPC_CODE = -32010;

type RequestId = string | number;

interface Responder {
  respond(id: RequestId, result: unknown): Promise<void>;
  respondError(
    id: RequestId,
    error: { code: number; message: string; data?: unknown },
  ): Promise<void>;
}

const OP_BY_METHOD = new Map<string, SerialToolOp>(
  SERIAL_TOOL_OPS.map((op) => [zcodeSerialMethods[op], op]),
);

function toToolError(error: unknown) {
  if (error instanceof SerialError) {
    return { code: SERIAL_TOOL_ERROR_RPC_CODE, message: error.message, data: { code: error.code } };
  }
  return {
    code: SERIAL_TOOL_ERROR_RPC_CODE,
    message: error instanceof Error ? error.message : String(error),
    data: { code: "io" },
  };
}

/**
 * Agent 运行时发来的 interaction/serial* 反向请求。返回 false 表示不是串口方法，交给后续分支。
 * Host 只做参数严格校验与路由，串口语义全部在 SerialAgentBridge 中。
 */
export function routeSerialAgentRequest(options: {
  request: { id: RequestId; method: string; params?: unknown };
  bridge: SerialAgentBridge | undefined;
  responder: Responder;
}): boolean {
  const { request, bridge, responder } = options;
  const reply = (promise: Promise<void>) => {
    promise.catch(() => {
      // 运行时连接已断开时应答会失败；请求方已不在，无需再处理。
    });
  };

  if (request.method === zcodeSerialCancelMethod) {
    const parsed = zcodeSerialCancelParamsSchema.safeParse(request.params);
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
    bridge?.cancel(parsed.data);
    reply(responder.respond(request.id, {}));
    return true;
  }

  const op = OP_BY_METHOD.get(request.method);
  if (!op) return false;
  const parsed = zcodeSerialMethodParamsSchemas[op].safeParse(request.params);
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
    reply(
      responder.respondError(
        request.id,
        toToolError(new SerialError("unavailable", "Serial port tools are not available here")),
      ),
    );
    return true;
  }
  reply(
    bridge
      .handle(op, parsed.data as never)
      .then((result) => responder.respond(request.id, result))
      .catch((error: unknown) => responder.respondError(request.id, toToolError(error))),
  );
  return true;
}
