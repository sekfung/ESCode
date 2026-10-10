import { randomUUID } from "node:crypto";
import {
  zcodeSerialCancelMethod,
  zcodeSerialMethodResultSchemas,
  zcodeSerialMethods,
} from "@zcode/shared/serial";
import { zcodeProtocolEmptyResultSchema } from "@zcode/shared";
import type { SerialControlPort } from "../app/serial-broker.js";
import {
  requireSession,
  type ParamsSchema,
  type ZCodeProtocolAgentServerContext,
} from "./server-types.js";

/**
 * Agent 侧串口端口：把 broker 请求转成 interaction/serial* 反向请求，由 Host 的 SerialService 执行。
 * 会话必须属于本运行时（requireSession）；workspace 身份字段随请求带给 Host，用于拒绝远程 workspace。
 */
export function createProtocolSerialControlPort(
  context: ZCodeProtocolAgentServerContext,
): SerialControlPort {
  return {
    async request(op, args, { sessionId, turnId, signal }) {
      const record = requireSession(context, sessionId, { operation: zcodeSerialMethods[op] });
      const workspaceIdentity = record.workspace.workspaceIdentity?.trim() || undefined;
      const remoteSessionId = record.workspace.remoteSessionId?.trim() || undefined;
      const workspacePath = record.workspace.workspacePath;
      const requestId = randomUUID();
      // 反向请求没有通用取消：工具调用被取消时显式通知 Host 结束等待并释放订阅（幂等）。
      const cancel = () => {
        void context
          .requestClient(
            zcodeSerialCancelMethod,
            { sessionId, targetRequestId: requestId },
            zcodeProtocolEmptyResultSchema,
          )
          .catch(() => undefined);
      };
      if (op === "waitFor") {
        if (signal?.aborted) cancel();
        else signal?.addEventListener("abort", cancel, { once: true });
      }
      try {
        return await context.requestClient(
          zcodeSerialMethods[op],
          {
            requestId,
            sessionId,
            ...(turnId ? { turnId } : {}),
            workspaceKey: workspaceIdentity ?? workspacePath,
            workspacePath,
            ...(workspaceIdentity ? { workspaceIdentity } : {}),
            ...(remoteSessionId ? { remoteSessionId } : {}),
            args,
          },
          // 按 op 索引得到的是各结果 schema 的联合；broker 只透传结果，这里统一按 unknown 校验后返回。
          zcodeSerialMethodResultSchemas[op] as ParamsSchema<unknown>,
          signal ? { signal } : undefined,
        );
      } finally {
        signal?.removeEventListener("abort", cancel);
      }
    },
  };
}
