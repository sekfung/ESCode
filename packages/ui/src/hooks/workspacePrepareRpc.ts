/**
 * workspace prepare 的协议 RPC 收口。
 *
 * 拆出原因：useWorkspacePrepare.ts 只保留可单测的轻量判定入口；
 * 这里只读取 workspace presentation（mode/slash commands）；模型选择事实由目标 Host View 提供。
 */
import type { IESCodeSessionService } from "@escode/services";
import { type ESCodeProvider, type ESCodeWorkspacePrepareResult } from "@escode/shared";
import { getChatErrorMessage } from "@/lib/chatPrepareError.js";
import { logger } from "@/logger.js";
import { escodeWorkspacePresentationToConfigOptions } from "@/lib/escodeSessionProjection.js";

export async function prepareWorkspaceWithESCodeSessionService(params: {
  workspacePath: string;
  workspaceIdentity?: string;
  provider: ESCodeProvider;
  escodeSessionService: Pick<IESCodeSessionService, "readWorkspacePresentation">;
}): Promise<ESCodeWorkspacePrepareResult> {
  const startedAt = Date.now();
  logger.info("[escode-workspace-presentation] workspace prepare start", {
    workspacePath: params.workspacePath,
    workspaceIdentity: params.workspaceIdentity ?? null,
    provider: params.provider,
  });

  let presentation: Awaited<ReturnType<IESCodeSessionService["readWorkspacePresentation"]>>;
  try {
    presentation = await params.escodeSessionService.readWorkspacePresentation({
      workspacePath: params.workspacePath,
      workspaceIdentity: params.workspaceIdentity,
    });
  } catch (error) {
    logger.warn("[escode-workspace-presentation] readWorkspacePresentation failed", {
      workspacePath: params.workspacePath,
      workspaceIdentity: params.workspaceIdentity ?? null,
      provider: params.provider,
      durationMs: Date.now() - startedAt,
      error: getChatErrorMessage(error),
    });
    throw error;
  }

  const readPresentationDurationMs = Date.now() - startedAt;
  const configOptions = escodeWorkspacePresentationToConfigOptions(presentation.mode);
  const totalDurationMs = Date.now() - startedAt;
  logger.info("[escode-workspace-presentation] readWorkspacePresentation done", {
    workspacePath: params.workspacePath,
    workspaceIdentity: params.workspaceIdentity ?? null,
    provider: params.provider,
    readPresentationDurationMs,
    totalDurationMs,
    configOptionsCount: configOptions.length,
    modeCurrent: presentation.mode,
  });

  return {
    executionCapabilities: presentation.executionCapabilities,
    workspacePath: params.workspacePath,
    preparedSessionId: "",
    version: "ESCode Protocol/1",
    provider: params.provider,
    configOptions,
    slashCommands: presentation.slashCommands,
  };
}
