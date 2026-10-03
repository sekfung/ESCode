/**
 * 工作流宿主的用户命令面（docs/specs/rust-v4-command-gaps.md）：Rust runtime 收到 V4 的 run 卡 /
 * 详情页操作后转到这里，语义与 Node runtime 的同名能力一致。
 * - `run.cancel {session, workId}`：TS runtime.cancelBackgroundTask——initiator = user，非 strict
 *   （已终态回 `alreadyTerminal`，由 Rust 映射成 not_running 拒绝）。
 * - `run.resume {session, cwd, workId, name?}`：TS app.resumeWorkflowRun——`port.resume` 成功后紧跟追踪重臂
 *   （合成 CreateWorkflow 描述子，与 core trackResumedDynamicWorkflowRun 同形）。
 */

/** CreateWorkflow：重臂追踪的描述子工具名（TS CREATE_WORKFLOW_TOOL_NAME，让通知/取消分派归 workflow）。 */
const CREATE_WORKFLOW_TOOL_NAME = "CreateWorkflow";
const RESUME_TOOL_CALL_PREFIX = "resume-";

export interface WorkflowRunCommandDeps {
  resume(session: string, runId: string): Promise<Record<string, unknown>>;
  stop(session: string, runId: string): Promise<Record<string, unknown>>;
  track(session: string, toolCall: Record<string, unknown>, output: Record<string, unknown>): Promise<unknown>;
  setWorkingDirectory(session: string, cwd: string): void;
  onTrackError(error: unknown): void;
}

export function isWorkflowRunCommand(method: string): boolean {
  return method === "run.cancel" || method === "run.resume";
}

export async function runWorkflowRunCommand(
  method: string,
  params: Record<string, any>,
  deps: WorkflowRunCommandDeps,
): Promise<unknown> {
  const session = String(params.session);
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
