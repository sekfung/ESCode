/**
 * 工作流宿主的用户命令面（docs/specs/rust-v4-command-gaps.md）：Rust runtime 收到 V4 的 run 卡 /
 * 详情页 / 中枢操作后转到这里，语义与 Node runtime 的同名能力一致。
 * - `run.cancel {session, workId}`：TS runtime.cancelBackgroundTask——initiator = user，非 strict
 *   （已终态回 `alreadyTerminal`，由 Rust 映射成 not_running 拒绝）。
 * - `run.resume {session, cwd, workId, name?}`：TS app.resumeWorkflowRun——`port.resume` 成功后紧跟追踪重臂
 *   （合成 CreateWorkflow 描述子，与 core trackResumedDynamicWorkflowRun 同形）。
 * - `run.startSaved {session, cwd, name, scope?, args?}`：中枢「运行」的零会话副作用段
 *   （core `launchSavedWorkflowRun`）——解析 / 实参校验 / 编译 / 工作副本 / `port.submit`，把启动轮需要
 *   的事实交回 Rust 落行。**不在这里重臂追踪**：追踪必须晚于启动轮，见 `run.track`。
 * - `run.track {session, toolCallId}`：为 `run.startSaved` 提交的 run 重臂后台追踪。Rust 在启动轮
 *   落定之后才发它，保证 run 的完成通知不会先于启动轮落成后台结果轮。
 */

/** CreateWorkflow：重臂追踪的描述子工具名（TS CREATE_WORKFLOW_TOOL_NAME，让通知/取消分派归 workflow）。 */
const CREATE_WORKFLOW_TOOL_NAME = "CreateWorkflow";
const RESUME_TOOL_CALL_PREFIX = "resume-";

export interface WorkflowRunCommandDeps {
  resume(session: string, runId: string): Promise<Record<string, unknown>>;
  stop(session: string, runId: string): Promise<Record<string, unknown>>;
  track(session: string, toolCall: Record<string, unknown>, output: Record<string, unknown>): Promise<unknown>;
  /** 直接启动已保存工作流的零会话副作用段（解析 / 校验 / 编译 / 工作副本 / submit）。 */
  startSaved(session: string, params: Record<string, any>): Promise<Record<string, unknown>>;
  /** 为 `run.startSaved` 提交的 run 重臂追踪（合描述子按 toolCallId 暂存）。 */
  trackSaved(session: string, toolCallId: string): Promise<Record<string, unknown>>;
  setWorkingDirectory(session: string, cwd: string): void;
  onTrackError(error: unknown): void;
}

export function isWorkflowRunCommand(method: string): boolean {
  return (
    method === "run.cancel" ||
    method === "run.resume" ||
    method === "run.startSaved" ||
    method === "run.track"
  );
}

export async function runWorkflowRunCommand(
  method: string,
  params: Record<string, any>,
  deps: WorkflowRunCommandDeps,
): Promise<unknown> {
  const session = String(params.session);
  // 直接启动：cwd 与 resume 同一条设置（run 的工作副本 / 相对路径都以它为准）。
  if (method === "run.startSaved") {
    if (typeof params.cwd === "string") deps.setWorkingDirectory(session, params.cwd);
    return deps.startSaved(session, params);
  }
  if (method === "run.track") {
    return deps.trackSaved(session, String(params.toolCallId));
  }
  const workId = String(params.workId);
  if (method === "run.cancel") {
    return deps.stop(session, workId);
  }
  if (typeof params.cwd === "string") deps.setWorkingDirectory(session, params.cwd);
  const result = await deps.resume(session, workId);
  if (result.ok === true) {
    const runId = String(result.runId);
    const toolCall = {
      id: typeof result.toolCallId === "string" ? result.toolCallId : `${RESUME_TOOL_CALL_PREFIX}${runId}`,
      name: CREATE_WORKFLOW_TOOL_NAME,
      input: typeof params.name === "string" ? { name: params.name } : {},
    };
    // 追踪重臂（registry 登记、终态 waiter、结算通知）与工具路径同一条：不阻塞命令应答。
    void deps
      .track(session, toolCall, { backgroundTaskId: runId, status: "backgrounded" })
      .catch(deps.onTrackError);
  }
  return result;
}
