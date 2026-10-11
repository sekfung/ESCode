import type { ESCodeSessionStateSnapshot } from "@escode/shared";
import { createServiceLogger } from "#src/logger/serviceLogger.js";
import { repairImportedClaudeSessionSnapshot } from "#src/session/claude-native/importedClaudeHistoryRepair.js";
import type { IESCodeAgentService } from "#src/escode-agent/escodeAgent.js";
import type {
  ESCodeSessionReadParams,
  ESCodeSessionResumeParams,
} from "#src/escode-session/escodeSession.js";

const logger = createServiceLogger("escode-session-service");

export async function repairEmptyImportedClaudeSessionSnapshot(params: {
  agentService: IESCodeAgentService;
  snapshot: ESCodeSessionStateSnapshot;
  target: ESCodeSessionResumeParams | ESCodeSessionReadParams;
}): Promise<ESCodeSessionStateSnapshot> {
  const repaired = await repairImportedClaudeSessionSnapshot({
    snapshot: params.snapshot,
    target: {
      workspacePath: params.target.workspacePath,
      workspaceIdentity: params.target.workspaceIdentity,
      taskId: params.target.sessionId,
      ...("mcpServers" in params.target && params.target.mcpServers
        ? { mcpServers: params.target.mcpServers }
        : {}),
    },
    createSession: (input) => params.agentService.createSession(input),
    onRepair: (history) => {
      logger.warn(
        undefined,
        `[escode-session-service] Claude 导入 session 历史异常，按 ${history.source} 回填 taskId=${params.target.sessionId}`,
      );
    },
  });
  return repaired ?? params.snapshot;
}
