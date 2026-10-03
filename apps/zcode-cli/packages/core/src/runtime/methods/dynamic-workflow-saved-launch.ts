import { randomUUID } from "node:crypto";
import {
  CREATE_WORKFLOW_TOOL_NAME,
  boundWorkflowLaunchMeta,
  createWorkflowPhaseAlongside,
  createWorkflowPhaseNames,
  type DynamicWorkflowRunPort,
  type SavedWorkflowScope,
  type TraceContext,
} from "@zcode/contracts";
import type { CompileDiagnostic } from "@zcode/dynamic-workflow";
import { uuidv7 } from "@zcode/shared";
import {
  resolveSavedWorkflow,
  validateWorkflowArgs,
} from "../../tool/handlers/saved-workflows/index.js";
import {
  boundGraphOfAnalysis,
  displayOfAnalysis,
} from "../../tool/handlers/workflow-analysis-display.js";
import { writeWorkflowDraft } from "../../tool/handlers/workflow-drafts.js";
import { analyzeScript } from "../../tool/handlers/workflow-script-analysis.js";
import type { ExecutableToolCall } from "../../tool/types.js";

/**
 * 中枢直接启动已保存工作流的「零会话副作用」段：解析 + 实参校验 + 编译 + 工作副本 + 提交 run，
 * 并铸出启动轮需要的全部事实（启动句、启动元数据、合成 CreateWorkflow 描述子）。
 *
 * 从 `startSavedWorkflowRun` 拆出，供两个宿主共用同一份实现：Node runtime 之后自己落启动轮与追踪；
 * Rust runtime 的工作流宿主（`__zcode-workflow-host`）把结果交回 Rust，由 Rust 落启动轮
 * （docs/specs/rust-v4-command-gaps.md）。`beforeSubmit` 在校验通过之后、提交之前调用（Node 在此
 * 初始化上下文并持久化父会话），失败即未提交。
 */
export type SavedWorkflowLaunchRejection = {
  ok: false;
  reason:
    | "invalid_name"
    | "not_found"
    | "invalid_args"
    | "compile_failed"
    | "session_busy"
    | "start_failed";
  message?: string;
};

export type SavedWorkflowLaunchResult =
  | {
      ok: true;
      runId: string;
      toolCallId: string;
      /** run 的 `run-launched.inputId`，同时是 controlOnly 启动轮的 inputId。 */
      launchInputId: string;
      /** 启动轮的模型面规范句（也是用户行文本）。 */
      launchText: string;
      /** 启动轮 turnHeader / userInput 共用的 workflowLaunch 元数据。 */
      meta: ReturnType<typeof boundWorkflowLaunchMeta>;
      /** 后台追踪用的合成 CreateWorkflow 描述子。 */
      toolCall: ExecutableToolCall;
      /** 会话标题种子（工作流名）。 */
      titleInput: string;
    }
  | SavedWorkflowLaunchRejection;

/** 编译诊断合并后的界（≈2KB）：诊断随 ACK 的 `message` 走协议，过长的堆栈没有阅读价值。 */
const COMPILE_DIAGNOSTICS_MAX_CHARS = 2_000;

export async function launchSavedWorkflowRun(input: {
  port: DynamicWorkflowRunPort | undefined;
  cwd: string;
  parentSessionId: string;
  name: string;
  scope?: SavedWorkflowScope;
  args?: Record<string, unknown>;
  traceContext: TraceContext;
  beforeSubmit?: (titleInput: string) => Promise<void>;
}): Promise<SavedWorkflowLaunchResult> {
  const { cwd } = input;
  // (1) 解析 + 实参校验：复用 create-workflow-source 的同一段归一化（第二个调用方，不复制）。
  const found = resolveSavedWorkflow({ cwd, name: input.name, scope: input.scope });
  if (!found.ok) {
    if (found.reason === "invalid_name") {
      return {
        ok: false,
        reason: "invalid_name",
        message: `'${input.name}' is not a usable workflow name: ${found.detail}`,
      };
    }
    if (found.reason === "not_found") {
      return {
        ok: false,
        reason: "not_found",
        message:
          input.scope === undefined
            ? `No saved workflow named '${input.name}' in this project or globally.`
            : `No ${input.scope} workflow named '${input.name}'.`,
      };
    }
    // parse_error / read_error：文件在但坏了。对 GUI 与「找不到」是同一个下一步（这个名字跑不
    // 起来），归 not_found；但 message 说清是文件问题而非名字问题，好让用户去修文件而不是改名字。
    return {
      ok: false,
      reason: "not_found",
      message: `The saved workflow '${input.name}' at ${found.path} could not be read: ${found.detail}`,
    };
  }

  const validated = validateWorkflowArgs(found.meta.args, input.args);
  if (!validated.ok) {
    return {
      ok: false,
      reason: "invalid_args",
      message: [
        `The arguments for saved workflow '${found.name}' are not valid:`,
        ...validated.errors.map((error) => `- ${error}`),
      ].join("\n"),
    };
  }

  // (2) 编译。任一诊断即拒绝——与 CreateWorkflow 对编不过脚本的处理同一条原则（弹一个注定失败的
  // run 只是延迟同一个错误）。诊断进 message，有界。
  const analysis = analyzeScript(found.script);
  if (!analysis.ok || analysis.diagnostics.length > 0) {
    return {
      ok: false,
      reason: "compile_failed",
      message: boundedCompileDiagnostics(
        `The saved workflow '${found.name}' has errors:`,
        analysis.diagnostics,
      ),
    };
  }

  // —— 到此为止零副作用：无 run、无消息、无事件、无任务。——

  const port = input.port;
  if (port === undefined) {
    // 端口缺席（stub / 单测宿主）：GUI 拿到能力不支持面。刻意不降级成「假装启动了」。
    return { ok: false, reason: "start_failed", message: "dynamic workflow port unavailable" };
  }

  await input.beforeSubmit?.(found.name);

  // (3) toolCallId：`launch-` 前缀，日志与工具卡可辨于模型工具调用 id（`tool_*`）与 resume 重臂。
  const toolCallId = `launch-${randomUUID()}`;
  const hasArgs = Object.keys(validated.args).length > 0;
  // 发起锚点：直接启动没有用户轮，铸一个 UUID v7 同时充当
  // run 的 `run-launched.inputId` 与 controlOnly 启动轮的 inputId——启动轮 run 卡与子代理的
  // agent_step 因此挂在同一个 message 下。
  const launchInputId = uuidv7();
  // 提交的声明阶段表读与启动轮 display 同一个「分析结果 → 有界显示图」投影（阶段来自控制流层，
  // 只传因果图就没有阶段），再过同一个 createWorkflowPhaseNames——两条路上同一脚本画同一条侧栏轨道。
  const launchGraph = boundGraphOfAnalysis(analysis);
  const phaseNames = createWorkflowPhaseNames(launchGraph);
  // 「同时在跑」表的下标指向 `phaseNames`，所以它必须来自同一张图、同一次投影。
  const phaseAlongside =
    phaseNames === undefined ? undefined : createWorkflowPhaseAlongside(launchGraph);

  // (3b) 工作副本。中枢直接启动与 `CreateWorkflow` 的 saved 来源是同一件事，所以拷贝也按同一条
  // 规矩写：逐字节（元数据块一起）、保存的定义本身一个字都不动。写不成就没有 `scriptPath`——run 照常起，
  // 只是终态通知里没有可编辑的文件可指。
  const draft = await writeWorkflowDraft({ cwd, name: found.name, source: found.source });

  // (4) 提交启动。提交失败（拒绝 / 抛错）在启动轮之前退出——绝不吞成带 runId 的成功。
  let runId: string;
  try {
    const submitted = await port.submit({
      scriptText: found.script,
      cwd,
      name: found.name,
      ...(hasArgs ? { args: validated.args } : {}),
      parentSessionId: input.parentSessionId,
      toolCallId,
      launchInputId,
      ...(phaseNames === undefined ? {} : { phaseNames }),
      ...(phaseAlongside === undefined ? {} : { phaseAlongside }),
      ...(draft === undefined ? {} : { scriptPath: draft.path }),
      trace: input.traceContext,
    });
    runId = submitted.runId;
  } catch (error) {
    return { ok: false, reason: "start_failed", message: describeError(error) };
  }

  // 图与脚本随启动轮走：run 详情侧板与轮尾 run 卡按 toolCallId 找「发起行」取 display.causalityGraph
  // 与 input.script，直接启动没有工具行，就把同一份 display 与脚本挂在启动元数据上。
  const display = displayOfAnalysis(analysis);
  const meta = boundWorkflowLaunchMeta({
    runId,
    toolCallId,
    name: found.name,
    scope: found.scope,
    path: found.path,
    ...(hasArgs ? { args: validated.args } : {}),
    description: found.meta.description,
    ...(display?.kind === "create_workflow" ? { display } : {}),
    script: found.script,
  });
  // 后台追踪的合成 CreateWorkflow 描述子：`input.name` 喂通知主题（workflowTaskSubject），
  // `name: CreateWorkflow` 让 per-tool 生命周期分派归 "workflow"。
  const toolCall: ExecutableToolCall = {
    id: toolCallId,
    name: CREATE_WORKFLOW_TOOL_NAME,
    input: {
      name: found.name,
      saved: {
        name: found.name,
        scope: found.scope,
        ...(hasArgs ? { args: validated.args } : {}),
      },
    },
  };
  return {
    ok: true,
    runId,
    toolCallId,
    launchInputId,
    launchText: buildLaunchMessageText(found.name, found.scope, runId, validated.args),
    meta,
    toolCall,
    titleInput: found.name,
  };
}

/**
 * 启动轮的模型面规范句（英文，不本地化——它进 provider transcript，是模型下一回合读到的东西）。
 * 有实参时把实参以 JSON 块附上；末句无条件劝阻重复启动（run 已在飞，进度以通知形式回来）。
 */
function buildLaunchMessageText(
  name: string,
  scope: SavedWorkflowScope,
  runId: string,
  args: Record<string, unknown>,
): string {
  const lines = [
    `Started the saved workflow "${name}" (${scope}) from the workflows hub as run ${runId}.`,
  ];
  if (Object.keys(args).length > 0) {
    lines.push("", "Arguments:", "```json", JSON.stringify(args, null, 2), "```");
  }
  lines.push("", "Progress and results arrive as background notifications; do not start it again.");
  return lines.join("\n");
}

/**
 * 编译诊断合并成一段有界文本（`L{line}:C{column} {message}` 逐条）。中枢启动与 GUI「配置」共用：
 * 两者都把诊断经 ACK 的 `message` 交给 GUI 的有界等宽块。
 */
export function boundedCompileDiagnostics(
  heading: string,
  diagnostics: readonly CompileDiagnostic[],
): string {
  const body = [
    heading,
    ...diagnostics.map(
      (diagnostic) => `L${diagnostic.line}:C${diagnostic.column} ${diagnostic.message}`,
    ),
  ].join("\n");
  return body.length > COMPILE_DIAGNOSTICS_MAX_CHARS
    ? `${body.slice(0, COMPILE_DIAGNOSTICS_MAX_CHARS - 1)}…`
    : body;
}

function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
