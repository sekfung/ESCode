/**
 * Rust runtime 的 V4 工作流只读查询（docs/specs/rust-dynamic-workflow.md M3）：`v4/conversation/workflowRun*`
 * 由 Rust 原样转到工作流宿主。参数校验、分页（多取一条判 hasMore）、钳制与结果 schema 逐字照 Node 的
 * v4-gateway；取数照 create-app 把这些读面映射到 run 服务端口的那一层。授权（run 属于该会话）在端口内。
 */

import {
  v4ConversationWorkflowRunArtifactDataParamsSchema,
  v4ConversationWorkflowRunArtifactDataResultSchema,
  v4ConversationWorkflowRunArtifactReadParamsSchema,
  v4ConversationWorkflowRunArtifactReadResultSchema,
  v4ConversationWorkflowRunArtifactsParamsSchema,
  v4ConversationWorkflowRunArtifactsResultSchema,
  v4ConversationWorkflowRunEventsParamsSchema,
  v4ConversationWorkflowRunEventsResultSchema,
  v4ConversationWorkflowRunNodeResultParamsSchema,
  v4ConversationWorkflowRunNodeResultResultSchema,
  v4ConversationWorkflowRunsParamsSchema,
  v4ConversationWorkflowRunsResultSchema,
  v4ConversationWorkflowRunWorkspaceParamsSchema,
  v4ConversationWorkflowRunWorkspaceResultSchema,
  WORKFLOW_ARTIFACT_LIMITS,
  WORKFLOW_WORKSPACE_LIMITS,
} from "@zcode/shared/zcode-protocol-v4";

/** 宿主转发的方法名（不带 `v4/conversation/` 前缀）。 */
export const WORKFLOW_QUERY_METHODS = new Set([
  "workflowRuns",
  "workflowRunEvents",
  "workflowRunArtifacts",
  "workflowRunArtifactData",
  "workflowRunArtifactRead",
  "workflowRunWorkspace",
  "workflowRunNodeResult",
]);

type Port = Record<string, any>;

export async function runWorkflowQuery(
  method: string,
  rawParams: unknown,
  port: (sessionId: string) => Port,
): Promise<unknown> {
  switch (method) {
    case "workflowRunEvents": {
      const params = v4ConversationWorkflowRunEventsParamsSchema.parse(rawParams);
      const limit = params.limit;
      const events = await port(params.sessionId).listEvents(params.runId, {
        ...(params.afterSequence === undefined ? {} : { afterSequence: params.afterSequence }),
        // 多取一条只为判定 hasMore；它不进结果页。
        ...(limit === undefined ? {} : { limit: limit + 1 }),
      });
      const hasMore = limit !== undefined && events.length > limit;
      return v4ConversationWorkflowRunEventsResultSchema.parse({
        events: hasMore ? events.slice(0, limit) : events,
        hasMore,
      });
    }
    case "workflowRuns": {
      const params = v4ConversationWorkflowRunsParamsSchema.parse(rawParams);
      const runs = await port(params.sessionId).listRunsForSession(params.limit);
      return v4ConversationWorkflowRunsResultSchema.parse({ runs });
    }
    case "workflowRunArtifacts": {
      const params = v4ConversationWorkflowRunArtifactsParamsSchema.parse(rawParams);
      const artifacts = await port(params.sessionId).listArtifacts(params.runId);
      return v4ConversationWorkflowRunArtifactsResultSchema.parse({ artifacts: artifacts ?? [] });
    }
    case "workflowRunArtifactData": {
      const params = v4ConversationWorkflowRunArtifactDataParamsSchema.parse(rawParams);
      const limit = Math.max(
        1,
        Math.min(
          params.limit ?? WORKFLOW_ARTIFACT_LIMITS.defaultItemsPerPage,
          WORKFLOW_ARTIFACT_LIMITS.maxItemsPerPage,
        ),
      );
      const items = await port(params.sessionId).listArtifactItems(params.runId, params.artifactId, {
        ...(params.afterSequence === undefined ? {} : { afterSequence: params.afterSequence }),
        limit: limit + 1,
      });
      const hasMore = items.length > limit;
      return v4ConversationWorkflowRunArtifactDataResultSchema.parse({
        items: hasMore ? items.slice(0, limit) : items,
        hasMore,
      });
    }
    case "workflowRunArtifactRead": {
      const params = v4ConversationWorkflowRunArtifactReadParamsSchema.parse(rawParams);
      const artifact = await port(params.sessionId).readArtifact(
        params.runId,
        params.artifactId,
        params.version,
      );
      if (artifact === undefined) {
        throw new Error(
          `fault.workflowRunArtifactRead.notFound: ${params.runId}/${params.artifactId}@${params.version}`,
        );
      }
      const bytes: Uint8Array = artifact.bytes;
      const totalBytes = bytes.byteLength;
      const start = Math.min(params.offset, totalBytes);
      const end = Math.min(start + params.limit, totalBytes);
      return v4ConversationWorkflowRunArtifactReadResultSchema.parse({
        dataBase64: Buffer.from(bytes.subarray(start, end)).toString("base64"),
        mediaType: artifact.contentType,
        totalBytes,
        nextOffset: end < totalBytes ? end : null,
      });
    }
    case "workflowRunWorkspace": {
      const params = v4ConversationWorkflowRunWorkspaceParamsSchema.parse(rawParams);
      const nodes = (await port(params.sessionId).listWorkspaceNodes(params.runId)) ?? [];
      const truncated = nodes.length > WORKFLOW_WORKSPACE_LIMITS.maxNodes;
      return v4ConversationWorkflowRunWorkspaceResultSchema.parse({
        nodes: truncated ? nodes.slice(0, WORKFLOW_WORKSPACE_LIMITS.maxNodes) : nodes,
        ...(truncated ? { truncated: true } : {}),
      });
    }
    case "workflowRunNodeResult": {
      const params = v4ConversationWorkflowRunNodeResultParamsSchema.parse(rawParams);
      const maxBytes = Math.max(
        1,
        Math.min(
          params.maxBytes ?? WORKFLOW_WORKSPACE_LIMITS.resultMaxBytes,
          WORKFLOW_WORKSPACE_LIMITS.resultMaxBytes,
        ),
      );
      const result = await port(params.sessionId).readWorkspaceNodeResult(
        params.runId,
        params.siteId,
        params.ordinal,
        { maxBytes },
      );
      if (result === undefined) {
        throw new Error(
          `fault.workflowRunNodeResult.notFound: ${params.runId}/${params.siteId}@${params.ordinal}`,
        );
      }
      return v4ConversationWorkflowRunNodeResultResultSchema.parse(result);
    }
    default:
      throw new Error(`Unsupported workflow query: ${method}`);
  }
}
