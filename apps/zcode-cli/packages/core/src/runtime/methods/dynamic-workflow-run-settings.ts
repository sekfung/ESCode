import { traceContextToLogContext } from "../deps.js";
import type { TraceContext } from "@zcode/contracts";
import type { AgentRuntimeInternal } from "../internal.js";
import {
  applyWorkflowRunSettings,
  type AmendWorkflowRunSettingsRejection,
} from "./dynamic-workflow-run-settings-apply.js";
import { enqueueSettingsTurn } from "./dynamic-workflow-run-settings-turn.js";

export { buildSettingsMessageText } from "./dynamic-workflow-run-settings-turn.js";
export type { AmendWorkflowRunSettingsRejection };

/**
 * GUI「配置」的请求。两项设置守工具的三态：
 * 省略 = 沿用，`null` = 回到默认（会话模型 / 本机上限），值 = 设定。
 */
export interface AmendWorkflowRunSettingsInput {
  runId: string;
  subagentModel?: string | null;
  maxConcurrency?: number | null;
  traceContext?: TraceContext;
}

export type AmendWorkflowRunSettingsResult =
  | { ok: true; runId: string; toolCallId: string; supersededRunId?: string }
  | { ok: false; reason: AmendWorkflowRunSettingsRejection; message?: string };

/**
 * GUI「配置」的 runtime 落地：判定与执行全在 {@link applyWorkflowRunSettings}（与工作流宿主共用
 * 同一段实现），这里只补会话侧的记账。
 *
 * 顺序固定：追踪重臂 → 设置轮入队。设置轮排进运行时队列（priority `next`），与通知同优先级；不
 * 先重臂的话，一个很快结算的 run 的完成通知会先于设置轮落成后台结果轮。
 *
 * 之后的失败只记日志不回滚：新 run 已在飞，可在侧板停下；撤回它反而制造孤儿。
 */
export async function amendWorkflowRunSettings(
  this: AgentRuntimeInternal,
  input: AmendWorkflowRunSettingsInput,
): Promise<AmendWorkflowRunSettingsResult> {
  const traceContext = input.traceContext ?? this.rootTraceContext;
  const applied = await applyWorkflowRunSettings(input, {
    sessionId: this.sessionId,
    workingDirectory: this.workingDirectory,
    modelCatalogPort: this.modelCatalogPort,
    port: this.dynamicWorkflowRunPort,
    traceContext,
  });
  if (!applied.ok) return applied;
  try {
    if (applied.registration !== undefined) {
      await this.executor.trackExternalBackgroundTask(
        applied.registration.toolCall,
        { backgroundTaskId: applied.registration.backgroundTaskId, status: "backgrounded" },
        traceContext,
        undefined,
      );
    }
    enqueueSettingsTurn.call(this, { ...applied.turn, traceContext });
  } catch (error) {
    const amending = applied.postPhase === "amend";
    this.logger?.error(
      amending
        ? "Workflow settings amended but post-amend bookkeeping failed"
        : "Workflow run retuned but the settings turn could not be queued",
      error instanceof Error ? error : new Error(String(error)),
      {
        ...traceContextToLogContext(traceContext),
        event: amending
          ? "dynamic_workflow.settings.post_amend_failed"
          : "dynamic_workflow.settings.post_retune_failed",
        module: "core.runtime",
        runId: applied.runId,
        toolCallId: applied.toolCallId,
      },
    );
  }
  return {
    ok: true,
    runId: applied.runId,
    toolCallId: applied.toolCallId,
    ...(applied.supersededRunId === undefined ? {} : { supersededRunId: applied.supersededRunId }),
  };
}
