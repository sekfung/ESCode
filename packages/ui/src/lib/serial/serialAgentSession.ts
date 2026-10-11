import type { ESCodeTaskMeta } from "@escode/shared";
import type { ESCodeSessionStoreState } from "@/store/escodeSessionStoreTypes.js";
import { getTaskMeta } from "@/store/escodeSessionStoreSelectors.js";

const SESSION_ID_FALLBACK_LENGTH = 8;

export interface SerialAgentSession {
  title?: string;
  workspacePath: string;
  workspaceIdentity?: string;
}

/**
 * 串口收发记录只携带 Agent 的 sessionId（= UI taskId）。在各 workspace 的任务缓存里查找标题与归属，
 * 供面板标注 `[Agent·标题]` 并跳转；查不到（会话未加载或已删除）时返回 null，由调用方回退显示 ID。
 */
export function resolveSerialAgentSession(
  workspaces: ESCodeSessionStoreState["workspaces"],
  sessionId: string,
): SerialAgentSession | null {
  for (const [workspaceKey, workspaceState] of Object.entries(workspaces)) {
    const task: ESCodeTaskMeta | null = getTaskMeta(workspaceState, sessionId);
    if (!task) continue;
    return {
      ...(task.title?.trim() ? { title: task.title.trim() } : {}),
      workspacePath: task.workspacePath ?? workspaceKey,
      ...(task.workspaceIdentity ? { workspaceIdentity: task.workspaceIdentity } : {}),
    };
  }
  return null;
}

export function formatSerialAgentLabel(title: string | undefined, sessionId: string): string {
  return title?.trim() || sessionId.slice(0, SESSION_ID_FALLBACK_LENGTH);
}
