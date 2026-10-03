import type { SavedWorkflowScope, TraceContext } from "@zcode/contracts";
import { createMessageId, traceContextToLogContext } from "../deps.js";
import type { AgentRuntimeInternal } from "../internal.js";
import { emitControlOnlyUserTurn, persistWorkflowLaunchUserMessage } from "./control-only-turn.js";
import {
  launchSavedWorkflowRun,
  type SavedWorkflowLaunchRejection,
} from "./dynamic-workflow-saved-launch.js";

// 解析 / 校验 / 编译 / 提交段与 Rust 的工作流宿主共用（dynamic-workflow-saved-launch.ts）。
export { boundedCompileDiagnostics } from "./dynamic-workflow-saved-launch.js";

/**
 * `startSavedWorkflowRun` 的结构化结果。成功给 run 的两把关联键（`runId` ≡ backgroundTaskId ≡
 * cancelBackgroundWork 的 workId；`toolCallId` 联工具卡 → 详情页）；失败走 `reason` 判别键
 * （house rule：错误码而非文本做流程判断），`message` 携带人可读原因供 GUI 行内展示。
 */
export type StartSavedWorkflowRunResult =
  | { ok: true; runId: string; toolCallId: string }
  | SavedWorkflowLaunchRejection;

/**
 * 中枢直接启动一个已保存的工作流。
 *
 * 它是 `port.submit` 的**第二个调用方**，与 `CreateWorkflow` 工具路径同构：同一段 saved 归一化
 * （`resolveSavedWorkflow` + `validateWorkflowArgs`，不复制）、同一个后台追踪器
 * （`trackExternalBackgroundTask` 喂一个合成 CreateWorkflow 描述子），产出的 run 对通知 / 取消 /
 * 恢复 / 详情侧板不可区分。区别只在：不经 `ToolExecutor.execute`（绕开权限判定 + `alwaysAsk`——
 * 这正是本特性的全部意义，用户在中枢里的点击就是同意），并以一条 controlOnly 「启动轮」把用户的
 * 真实动作落进会话（模型直到完成 / 提问通知到来才第一次听说这次 run）。
 *
 * 顺序固定，且解析 / 校验 / 编译失败在**任何持久化之前**（无 run、无消息、无事件、无任务）；提交之后的
 * 失败只记日志不回滚（run 已在飞、可在侧板取消），仍返回 ok。
 */
export async function startSavedWorkflowRun(
  this: AgentRuntimeInternal,
  input: {
    name: string;
    scope?: SavedWorkflowScope;
    args?: Record<string, unknown>;
    traceContext?: TraceContext;
  },
): Promise<StartSavedWorkflowRunResult> {
  const traceContext = input.traceContext ?? this.rootTraceContext;

  // (0) 忙碌会话拒绝。本方法只对刚建的空会话有意义；把启动排进活动 turn 的队列需要 controlOnly
  // 轮与 provider grammar 协调（属「加入当前会话」的未来工作）。GUI 只对空会话发它，故正常不触发。
  if (this.hasActiveOrQueuedTurnWork()) {
    return { ok: false, reason: "session_busy" };
  }

  const launched = await launchSavedWorkflowRun({
    port: this.dynamicWorkflowRunPort,
    cwd: this.workingDirectory,
    parentSessionId: this.sessionId,
    name: input.name,
    ...(input.scope === undefined ? {} : { scope: input.scope }),
    ...(input.args === undefined ? {} : { args: input.args }),
    traceContext,
    // 提交前先初始化上下文并持久化父会话。actor 的 session_task_link 通过
    // parent_session_id 引用父会话；父行不存在时，首个 actor 的创建会因外键约束失败。
    // 父会话以工作流名为首输入标题，后续启动轮会幂等复用。提交失败时由 GUI 在收到
    // rejected ACK 后通过 deleteSession 回收空会话。
    beforeSubmit: async (titleInput) => {
      await this.ensureContextInitialized(traceContext);
      await this.ensureSessionPersisted(titleInput, traceContext);
    },
  });
  if (!launched.ok) return launched;
  const { runId, toolCallId, launchText, meta, toolCall } = launched;

  // 提交之后的失败只记日志不回滚：run 已在飞，可在侧板取消；把它撤回反而制造一个无归属的孤儿 run。
  try {
    // (5) 启动轮：user 可见消息（synthetic + workflowLaunch 元数据）+ history + controlOnly turn
    // 边界；会话标题 = 工作流名（ensureSessionPersisted 以名为首输入标题）。
    const messageId = createMessageId();
    await emitControlOnlyUserTurn.call(this, {
      messageId,
      titleInput: launched.titleInput,
      historyText: launchText,
      turnInput: launchText,
      traceContext,
      inputId: launched.launchInputId,
      inputSource: "workflow_launch",
      workflowLaunch: meta,
      persistMessage: () =>
        persistWorkflowLaunchUserMessage.call(this, {
          messageID: messageId,
          text: launchText,
          meta,
          traceContext,
        }),
    });

    // (6) 后台追踪：合成一个 CreateWorkflow 描述子走 executor 的同一条 trackBackgroundTask
    // （runtime-task registry 登记 = 会话回收护栏、BackgroundTaskStarted、终态 waiter、结算通知）。
    await this.executor.trackExternalBackgroundTask(
      toolCall,
      { backgroundTaskId: runId, status: "backgrounded" },
      traceContext,
      undefined,
    );
  } catch (error) {
    this.logger?.error(
      "Saved workflow launched but post-submit bookkeeping failed",
      error instanceof Error ? error : new Error(String(error)),
      {
        ...traceContextToLogContext(traceContext),
        event: "dynamic_workflow.launch.post_submit_failed",
        module: "core.runtime",
        runId,
        toolCallId,
      },
    );
  }

  return { ok: true, runId, toolCallId };
}
