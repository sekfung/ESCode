import { randomUUID } from "node:crypto";
import { circuitToolResponseSchemas, escodeCircuitMethods } from "@escode/shared/circuit";
import type { CircuitControlPort } from "../app/circuit-broker.js";
import {
  requireSession,
  type ParamsSchema,
  type ESCodeProtocolAgentServerContext,
} from "./server-types.js";

/**
 * Agent 侧电路端口：把 broker 请求转成 interaction/circuit* 反向请求，由持有 CircuitService 的 Host 执行。
 * 会话必须属于本运行时（requireSession）；workspace 身份与 remoteSessionId 随请求带给 Host（§3.5），
 * 这样远程 workspace 的 IR 只会落在持有它的 Host 上。
 */
export function createProtocolCircuitControlPort(
  context: ESCodeProtocolAgentServerContext,
): CircuitControlPort {
  return {
    async request(op, args, { sessionId, turnId, signal }) {
      const record = requireSession(context, sessionId, { operation: escodeCircuitMethods[op] });
      const workspaceIdentity = record.workspace.workspaceIdentity?.trim() || undefined;
      const remoteSessionId = record.workspace.remoteSessionId?.trim() || undefined;
      const workspacePath = record.workspace.workspacePath;
      return await context.requestClient(
        escodeCircuitMethods[op],
        {
          requestId: randomUUID(),
          sessionId,
          ...(turnId ? { turnId } : {}),
          workspaceKey: workspaceIdentity ?? workspacePath,
          workspacePath,
          ...(workspaceIdentity ? { workspaceIdentity } : {}),
          ...(remoteSessionId ? { remoteSessionId } : {}),
          args,
        },
        // 五个 op 的响应都是工具信封（成功或失败）；broker 只透传结果，这里统一校验后返回。
        circuitToolResponseSchemas[op] as ParamsSchema<unknown>,
        signal ? { signal } : undefined,
      );
    },
  };
}
