/**
 * 中枢直接启动已保存工作流的宿主两段（`run.startSaved` / `run.track`，
 * docs/specs/rust-v4-command-gaps.md「startSavedWorkflow」）。从 `workflow-host-command` 拆出：
 * 那份命令面已接近单文件 400 行上限。
 *
 * `run.startSaved` 只做零会话副作用段（解析 / 实参校验 / 编译 / 工作副本 / `port.submit`）——启动轮由
 * Rust 的会话 owner 落，追踪必须晚于启动轮，所以提交后的合成 CreateWorkflow 描述子在这里暂存，等
 * `run.track` 认领（认领即删除）。宿主进程退出即失效；届时 Rust 侧的 run 也已随宿主结束。
 */
import { launchSavedWorkflowRun } from "@zcode/core";

/** 找不到描述子的 `run.track`：Rust 只记 warn，不回滚已提交的 run。 */
const UNKNOWN_TOOL_CALL = "unknown_tool_call";

export interface SavedWorkflowLaunchDeps {
  /** 会话的 run 服务端口（`undefined` 表示该会话尚未初始化 run 服务，与 Node 同一判定）。 */
  port: (session: string) => Parameters<typeof launchSavedWorkflowRun>[0]["port"];
  /** 后台追踪重臂（与 `run.resume` 同一条 BackgroundTaskTracker；调用方不等待命令应答）。 */
  track: (session: string, toolCall: Record<string, unknown>, runId: string) => Promise<unknown>;
}

export function createSavedWorkflowLaunch(deps: SavedWorkflowLaunchDeps) {
  /** `toolCallId` → 合成描述子 + runId：`run.startSaved` 与 `run.track` 之间的桥。 */
  const launchedRuns = new Map<string, { runId: string; toolCall: Record<string, unknown> }>();
  return {
    startSaved: async (
      session: string,
      params: Record<string, any>,
    ): Promise<Record<string, unknown>> => {
      const launched = await launchSavedWorkflowRun({
        port: deps.port(session),
        cwd: typeof params.cwd === "string" ? params.cwd : process.cwd(),
        parentSessionId: session,
        name: String(params.name),
        ...(params.scope === undefined ? {} : { scope: params.scope }),
        ...(params.args === undefined ? {} : { args: params.args }),
        // run 的发起锚点：Rust 侧没有可复用的进程内 trace，铸一个以会话为界的一次性 trace。
        traceContext: { traceId: `saved-workflow-launch:${session}` } as never,
      });
      if (!launched.ok) return launched as unknown as Record<string, unknown>;
      launchedRuns.set(launched.toolCallId, {
        runId: launched.runId,
        toolCall: launched.toolCall as unknown as Record<string, unknown>,
      });
      return {
        ok: true,
        runId: launched.runId,
        toolCallId: launched.toolCallId,
        launchInputId: launched.launchInputId,
        launchText: launched.launchText,
        meta: launched.meta,
        titleInput: launched.titleInput,
      };
    },
    trackSaved: async (session: string, toolCallId: string): Promise<Record<string, unknown>> => {
      const launched = launchedRuns.get(toolCallId);
      if (launched === undefined) return { ok: false, reason: UNKNOWN_TOOL_CALL };
      launchedRuns.delete(toolCallId);
      await deps.track(session, launched.toolCall, launched.runId);
      return { ok: true };
    },
  };
}
