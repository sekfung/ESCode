import type { IESCodeSessionService } from "@escode/services";
import { logger } from "@/logger.js";
import { useESCodeSessionStore } from "@/store/escodeSessionStore.js";

interface DeferredDraftRuntimeChangeParams {
  logScope: string;
  reason: string;
  workspacePath?: string | null;
  workspaceIdentity?: string | null;
  /** 过渡期接受旧调用方传入；本函数只发失效版本，不直接操作 session service。 */
  escodeSessionService?: IESCodeSessionService;
}

export async function invalidateDeferredDraftSessionForRuntimeChange(
  params: DeferredDraftRuntimeChangeParams,
): Promise<void> {
  if (!params.workspacePath) {
    return;
  }

  const workspaceIdentity = params.workspaceIdentity?.trim() || undefined;
  const store = useESCodeSessionStore.getState();
  // V4 pane 是预热会话的唯一 owner；设置操作只发失效版本，不直接关闭会话。
  store.invalidateDraftRuntime(params.workspacePath, workspaceIdentity);
  logger.info(`[${params.logScope}] invalidated draft runtime capabilities`, {
    reason: params.reason,
    workspaceIdentity: workspaceIdentity ?? null,
    workspacePath: params.workspacePath,
  });
}

export async function invalidateDeferredDraftSessionForSkillChange(
  params: Omit<DeferredDraftRuntimeChangeParams, "logScope">,
): Promise<void> {
  await invalidateDeferredDraftSessionForRuntimeChange({ ...params, logScope: "skills" });
}
