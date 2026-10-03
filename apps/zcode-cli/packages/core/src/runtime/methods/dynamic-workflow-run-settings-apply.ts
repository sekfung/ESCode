// ============================================================
// GUI「配置」的判定与执行本体（不含会话侧记账）
// ============================================================
// 从 dynamic-workflow-run-settings.ts 拆出：那边只剩绑定 AgentRuntimeInternal 的薄壳，这里是
// 两个调用方共用的一段——Node runtime（拿会话事实与执行器）与 Rust 的工作流宿主
// （`run.amendSettings`，拿 cwd、run 端口与 Rust 递来的模型目录）。
//
// 会话侧的记账刻意留在调用方：后台追踪重臂要执行器，设置轮要入队或落行，两者在两侧的实现不同，
// 而「判定 + 执行」必须逐字相同。四条副作用顺序（追踪 → 设置轮）由调用方按下述返回值复现。

import { randomUUID } from "node:crypto";
import {
  AMEND_WORKFLOW_TOOL_NAME,
  boundWorkflowLaunchMeta,
  createWorkflowPhaseAlongside,
  createWorkflowPhaseNames,
  type DynamicWorkflowRunPort,
  type DynamicWorkflowRunRetuneResult,
  type ModelCatalogPort,
  type TraceContext,
  type WorkflowSettingsAmendMeta,
} from "@zcode/contracts";
import {
  resolveAmendMaxConcurrency,
  resolveAmendSubagentModelChoice,
} from "../../tool/handlers/amend-workflow-resolve.js";
import { resolveKeptScriptFile } from "../../tool/handlers/amend-workflow-source.js";
import { parseWorkflowSubagentModel } from "../../tool/handlers/model-reference.js";
import {
  boundGraphOfAnalysis,
  displayOfAnalysis,
} from "../../tool/handlers/workflow-analysis-display.js";
import {
  resolveWorkflowDraftName,
  writeWorkflowDraft,
} from "../../tool/handlers/workflow-drafts.js";
import { analyzeScript } from "../../tool/handlers/workflow-script-analysis.js";
import type { ExecutableToolCall } from "../../tool/types.js";
import { boundedCompileDiagnostics } from "./dynamic-workflow-run-start.js";
import {
  buildSettingsMessageText,
  fromTo,
  runSettingsOfSnapshot,
  type RunSettings,
} from "./dynamic-workflow-run-settings-turn.js";

/** 拒绝词表与 shared 的 `workflowRunSettingsRejectionReasonSchema` 逐字相同。 */
export type AmendWorkflowRunSettingsRejection =
  | "not_found"
  | "not_configurable"
  | "unchanged"
  | "script_missing"
  | "model_unavailable"
  | "compile_failed"
  | "missing_boundaries"
  | "start_failed";

/** 两项设置守工具的三态：省略 = 沿用，`null` = 回到默认（会话模型 / 本机上限），值 = 设定。 */
export interface WorkflowRunSettingsRequest {
  runId: string;
  subagentModel?: string | null;
  maxConcurrency?: number | null;
}

/** 调用方注入的宿主事实。`port` 与 Node runtime 的 `dynamicWorkflowRunPort` 同一形状。 */
export interface WorkflowRunSettingsApplyDeps {
  /** run 的归属判据（只改本会话名下的 run）。 */
  sessionId: string;
  /** 脚本草稿与「沿用脚本文件」的解析基准。 */
  workingDirectory: string;
  /** 缺席即「宿主解不了模型名」，与 Node 端口缺席同一条判定。 */
  modelCatalogPort?: ModelCatalogPort;
  port: DynamicWorkflowRunPort | undefined;
  /** run 的发起锚点（`port.amend` 的 `trace`）。 */
  traceContext: TraceContext;
}

export type WorkflowRunSettingsApplyResult =
  | { ok: false; reason: AmendWorkflowRunSettingsRejection; message?: string }
  | {
      ok: true;
      runId: string;
      toolCallId: string;
      supersededRunId?: string;
      /**
       * 非就地生效时要重臂的后台追踪。**就地调并发时缺席**：那个 run 本来就在追踪器里，再登记一次
       * 会按 AmendWorkflow 的 rearm 规则把一个从没停过的 run 的结算面清空。
       */
      registration?: { toolCall: ExecutableToolCall; backgroundTaskId: string };
      /** 设置轮的会话记录（调用方负责排队 / 落行）。 */
      turn: {
        text: string;
        meta: ReturnType<typeof boundWorkflowLaunchMeta>;
        titleInput: string;
      };
      /** 记账阶段：`amend` 起了新 run，`retune` 就地生效。调用方的失败日志按它分叉。 */
      postPhase: "amend" | "retune";
    };

/**
 * GUI「配置」：以同一份脚本、新的设置修订一个 run。
 *
 * 它是 `port.amend` 与 `port.retuneConcurrency` 的**第二个调用方**，与 `AmendWorkflow` 工具同构：
 * 同一段三态归一（`resolveAmendSubagentModelChoice` / `resolveAmendMaxConcurrency`，不复制）、同一条
 * 路由、同一个编译、同一个后台追踪描述子。区别只在：不经模型轮、不开确认窗（用户在弹层里点「应用」
 * 就是同意，脚本也是他批准过的那一份），实参沿用前驱（`inheritArgs`），并用一条设置轮把这件事记进
 * 会话。
 *
 * 只改并发、run 又还在飞时**就地生效**：同一个 runId、不停、不铸后继。
 *
 * 顺序固定：直到 `port.amend` / `port.retuneConcurrency` 之前的每一步失败都是零副作用——旧 run
 * 照旧在跑、没有新行、没有消息。
 */
export async function applyWorkflowRunSettings(
  input: WorkflowRunSettingsRequest,
  deps: WorkflowRunSettingsApplyDeps,
): Promise<WorkflowRunSettingsApplyResult> {
  const port = deps.port;
  if (port === undefined || typeof port.amend !== "function") {
    // 能力只在端口带 amend 与 getScript 时注册；到这里还缺，是接线故障而不是用户输入。
    return { ok: false, reason: "start_failed", message: "dynamic workflow amend unavailable" };
  }

  // (1) 这个 run 必须存在、且属于本会话——命令发给哪个会话，就只能改那个会话自己的 run。
  const snapshot = await port.getTask(input.runId);
  if (snapshot === undefined || snapshot.parentSessionId !== deps.sessionId) {
    return { ok: false, reason: "not_found" };
  }
  // (2) 已完成的 run 每个 ask 都会从缓存重放，新设置无处生效；被替代的 run 活的是它的后继。
  if (snapshot.runStatus === "completed" || snapshot.supersededBy !== undefined) {
    return { ok: false, reason: "not_configurable" };
  }
  // (3)(4) 两项设置的三态归一，与工具同一段代码。
  const current = runSettingsOfSnapshot(snapshot);
  const model = resolveAmendSubagentModelChoice(
    input.subagentModel,
    current.subagentModel,
    deps.modelCatalogPort,
  );
  if (!model.ok) {
    // 目录缺席时的那句话是写给模型的（「omit subagent_model」），GUI 只要原因码；有目录时的
    // 解析诊断（候选名等）对人同样有用，随 message 走。
    return {
      ok: false,
      reason: "model_unavailable",
      ...(deps.modelCatalogPort === undefined ? {} : { message: model.message }),
    };
  }
  const ceiling = port.concurrencyCeiling?.();
  const bound = resolveAmendMaxConcurrency(input.maxConcurrency, current.maxConcurrency, ceiling);
  // 等于天花板的界就是「没有自己的界」：快照只在低于天花板时带 maxConcurrency，两边同一个读法，
  // 「未改」的比较才成立（弹层把步进器推到顶也发 null，这里兜住直接给数的调用方）。
  const nextBound =
    bound.max_concurrency === undefined || bound.max_concurrency === ceiling
      ? undefined
      : bound.max_concurrency;
  const next: RunSettings = {
    ...(model.canonical === undefined ? {} : { subagentModel: model.canonical }),
    ...(nextBound === undefined ? {} : { maxConcurrency: nextBound }),
  };
  // (5) 什么都没变就不起新 run：一次修订会停下在飞的 run，没有理由为零改动付这个代价。
  const modelChanged = next.subagentModel !== current.subagentModel;
  const boundChanged = next.maxConcurrency !== current.maxConcurrency;
  if (!modelChanged && !boundChanged) return { ok: false, reason: "unchanged" };

  // (6) 分叉：只有并发变了、run 又在飞，就地生效——同一个 runId，不停、不铸后继、不导入缓存。
  // 端口答 not_live（已结算，或 pending 但引擎还没建）就顺着这张表往下走，落成今天那次修订。
  if (boundChanged && !modelChanged && typeof port.retuneConcurrency === "function") {
    // 弹层的 `null` 原样递到端口：天花板那个数只有端口知道，这里不猜第二遍。缺席只可能来自
    // 「沿用的界高过本机天花板」（run 是在更大的机器上起的），那时要的也正是回到天花板。
    const answer = await port.retuneConcurrency({
      runId: input.runId,
      maxConcurrency: input.maxConcurrency ?? null,
    });
    if (answer.ok) {
      return retunedSettings({
        answer,
        name: displayNameOfSnapshot(snapshot),
        runId: input.runId,
      });
    }
    if (answer.reason === "unchanged") return { ok: false, reason: "unchanged" };
  }

  // (7) 沿用的脚本（「Keeping the predecessor's script」同一条读路）。读在分叉**之后**：就地调
  // 并发跑的还是同一段脚本，一个没有存档脚本、或脚本已经编不过的 run 因此照样调得动上界。
  const script =
    typeof port.getScript === "function" ? await port.getScript(input.runId) : undefined;
  if (script === undefined || script.length === 0) {
    return { ok: false, reason: "script_missing" };
  }

  // (8) 编译。存下的脚本可能是在更早的 facade 上写的；编不过就停在这里，旧 run 不动。
  const analysis = analyzeScript(script);
  if (!analysis.ok || analysis.diagnostics.length > 0) {
    return {
      ok: false,
      reason: "compile_failed",
      message: boundedCompileDiagnostics(
        `The stored script of run ${input.runId} has errors:`,
        analysis.diagnostics,
      ),
    };
  }

  // —— 到此为止零副作用。——
  //
  // 新 run 的脚本文件，与工具沿用脚本时同一条规则、同一段代码：前驱的脚本文件此刻仍是这份字节就
  // 继续记它，否则照「不来自文件的脚本」写一份新草稿。不记的话，模型之后要修订这个 run 就只剩把
  // 整份脚本内联再抄一遍这一条路。草稿尽力而为：写不成即缺席，run 照常起。
  const graph = boundGraphOfAnalysis(analysis);
  const keptFile = await resolveKeptScriptFile({
    cwd: deps.workingDirectory,
    scriptPath: snapshot.scriptPath,
    script,
  });
  const scriptPath =
    keptFile?.path ??
    (
      await writeWorkflowDraft({
        cwd: deps.workingDirectory,
        name: resolveWorkflowDraftName(snapshot.name, graph),
        source: script,
      })
    )?.path;

  // 修订。`settings-` 前缀让日志与卡片分得出它与模型工具调用（`tool_*`）、中枢启动（`launch-`）。
  const toolCallId = `settings-${randomUUID()}`;
  const phaseNames = createWorkflowPhaseNames(graph);
  const phaseAlongside = phaseNames === undefined ? undefined : createWorkflowPhaseAlongside(graph);
  const subagentModel = parseWorkflowSubagentModel(next.subagentModel);
  let amended: Awaited<ReturnType<NonNullable<typeof port.amend>>>;
  try {
    amended = await port.amend({
      scriptText: script,
      cwd: deps.workingDirectory,
      predecessorRunId: input.runId,
      parentSessionId: deps.sessionId,
      toolCallId,
      ...(phaseNames === undefined ? {} : { phaseNames }),
      ...(phaseAlongside === undefined ? {} : { phaseAlongside }),
      ...(next.maxConcurrency === undefined ? {} : { maxConcurrency: next.maxConcurrency }),
      ...(subagentModel === undefined ? {} : { subagentModel }),
      ...(scriptPath === undefined ? {} : { scriptPath }),
      // 重跑的是前驱自己的脚本，它读的正是前驱启动时的实参。
      inheritArgs: true,
      trace: deps.traceContext,
    });
  } catch (error) {
    return {
      ok: false,
      reason: "start_failed",
      message: error instanceof Error ? error.message : String(error),
    };
  }
  if (!amended.ok) {
    return {
      ok: false,
      reason: amended.reason === "run_not_found" ? "not_found" : "missing_boundaries",
    };
  }

  // 名字只取 run 自己的：没起过名的 run 就不带名字，卡片、侧板与通知照任何无名 run 的规矩换用兜底词。
  // 这里曾兜底成 `run <id>`，设置轮的卡片与侧板标题于是成了一串 run id，
  // 而且这个假名会沿修订链一路传下去。
  const name = displayNameOfSnapshot(snapshot);
  const amend: WorkflowSettingsAmendMeta = {
    predecessorRunId: input.runId,
    ...(modelChanged ? { subagentModel: fromTo(current.subagentModel, next.subagentModel) } : {}),
    ...(boundChanged
      ? { maxConcurrency: fromTo(current.maxConcurrency, next.maxConcurrency) }
      : {}),
    ...(ceiling === undefined ? {} : { ceiling }),
  };
  // 后台追踪的合成 AmendWorkflow 描述子：`input.name` 喂通知主题，工具名让分派归 "workflow"。
  const toolCall: ExecutableToolCall = {
    id: toolCallId,
    name: AMEND_WORKFLOW_TOOL_NAME,
    input: { run_id: input.runId, ...(name === undefined ? {} : { name }) },
  };
  const display = displayOfAnalysis(analysis);
  return {
    ok: true,
    runId: amended.runId,
    toolCallId,
    ...(amended.supersededRunId === undefined ? {} : { supersededRunId: amended.supersededRunId }),
    registration: { toolCall, backgroundTaskId: amended.runId },
    turn: {
      text: buildSettingsMessageText({
        ...(name === undefined ? {} : { name }),
        previous: input.runId,
        runId: amended.runId,
        superseded: amended.supersededRunId !== undefined,
        amend,
      }),
      meta: boundWorkflowLaunchMeta({
        runId: amended.runId,
        toolCallId,
        ...(name === undefined ? {} : { name }),
        ...(display?.kind === "create_workflow" ? { display } : {}),
        amend,
      }),
      // 会话此时必已落库（它名下有 run），标题种子不会被用上；给一个诚实的值即可。
      titleInput: name ?? input.runId,
    },
    postPhase: "amend",
  };
}

/**
 * 就地生效的收尾：同一个 runId、没有 `supersededRunId`、**不登记第二个后台任务**。
 *
 * 设置轮照记，但 `amend` 块不带 `predecessorRunId`：缺席即「就地生效」，渲染端据此只画一行、
 * 不再画一张 run 卡（同一个 run 两张卡会读成两次运行）。也没有 `display`——这条路不编译。
 */
function retunedSettings(options: {
  answer: Extract<DynamicWorkflowRunRetuneResult, { ok: true }>;
  name: string | undefined;
  runId: string;
}): WorkflowRunSettingsApplyResult {
  const { answer, name, runId } = options;
  const toolCallId = `settings-${randomUUID()}`;
  // 等于天花板的那一端就是「默认」，于是整端缺席——与修订那条路同一个读法（弹层把步进器推到顶
  // 发的是 `null`，端口答回来的却永远是绝对值，折算只能在这里做）。
  const amend: WorkflowSettingsAmendMeta = {
    maxConcurrency: fromTo(
      answer.previous === answer.ceiling ? undefined : answer.previous,
      answer.maxConcurrency === answer.ceiling ? undefined : answer.maxConcurrency,
    ),
    ceiling: answer.ceiling,
  };
  return {
    ok: true,
    runId,
    toolCallId,
    turn: {
      text: buildSettingsMessageText({
        ...(name === undefined ? {} : { name }),
        previous: runId,
        runId,
        superseded: false,
        amend,
      }),
      meta: boundWorkflowLaunchMeta({
        runId,
        toolCallId,
        ...(name === undefined ? {} : { name }),
        amend,
      }),
      titleInput: name ?? runId,
    },
    postPhase: "retune",
  };
}

/** run 自己的名字（无名即缺席）：设置轮与合成追踪都按这一条，绝不拿 run id 当标题。 */
function displayNameOfSnapshot(snapshot: { name?: string }): string | undefined {
  const trimmed = snapshot.name?.trim();
  return trimmed === undefined || trimmed.length === 0 ? undefined : trimmed;
}
