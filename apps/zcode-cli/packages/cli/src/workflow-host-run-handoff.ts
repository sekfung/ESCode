/**
 * 工作流宿主的「提交 → 追踪重臂」两段桥（docs/specs/rust-v4-command-gaps.md）。从
 * `workflow-host-command` 拆出：那份命令面已接近单文件 400 行上限。
 *
 * 两类命令共用同一张 `toolCallId → 合成追踪描述子` 表，等 `run.track` 认领（认领即删除）。共同的
 * 时序约束是**追踪必须晚于 Rust 侧落轮**——两类命令各自的轮都由会话 owner 落，宿主只提交 run：
 * - `run.startSaved`：中枢直接启动已保存的工作流，只做零会话副作用段（解析 / 实参校验 / 编译 /
 *   工作副本 / `port.submit`），结果交回 Rust 落启动轮。
 * - `run.amendSettings`：GUI「配置」的修订，判定与执行全在 core 的 `applyWorkflowRunSettings`
 *   （与 Node runtime 同一段代码），设置轮同样由 Rust 落。就地调并发那条路没有新 run，因而不入表。
 *
 * 宿主进程退出即失效；届时 Rust 侧的 run 也已随宿主结束。
 */
import { applyWorkflowRunSettings, launchSavedWorkflowRun } from "@zcode/core";

/** 找不到描述子的 `run.track`：Rust 只记 warn，不回滚已提交的 run。 */
const UNKNOWN_TOOL_CALL = "unknown_tool_call";

/** 合成 trace 的会话锚点前缀：宿主没有可复用的进程内 trace，铸一个以会话为界的一次性 trace。 */
const LAUNCH_TRACE_PREFIX = "saved-workflow-launch:";
const SETTINGS_TRACE_PREFIX = "workflow-run-settings:";

export interface WorkflowRunHandoffDeps {
  /** 会话的 run 服务端口（`undefined` 表示该会话尚未初始化 run 服务，与 Node 同一判定）。 */
  port: (session: string) => Parameters<typeof launchSavedWorkflowRun>[0]["port"];
  /** 后台追踪重臂（与 `run.resume` 同一条 BackgroundTaskTracker；调用方不等待命令应答）。 */
  track: (session: string, toolCall: Record<string, unknown>, runId: string) => Promise<unknown>;
}

export function createWorkflowRunHandoff(deps: WorkflowRunHandoffDeps) {
  /** `toolCallId` → 合成描述子 + runId：各 `run.*` 命令与 `run.track` 之间的桥。 */
  const pending = new Map<string, { runId: string; toolCall: Record<string, unknown> }>();
  const stash = (toolCallId: string, runId: string, toolCall: unknown): void => {
    pending.set(toolCallId, { runId, toolCall: toolCall as Record<string, unknown> });
  };
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
        traceContext: { traceId: `${LAUNCH_TRACE_PREFIX}${session}` } as never,
      });
      if (!launched.ok) return launched as unknown as Record<string, unknown>;
      stash(launched.toolCallId, launched.runId, launched.toolCall);
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
    amendSettings: async (
      session: string,
      params: Record<string, any>,
    ): Promise<Record<string, unknown>> => {
      // 模型目录由 Rust 的 Provider Registry 递来（它的注册表不在本进程）。缺席即「宿主解不了模型
      // 名」，与 Node 端口缺席同一条判定。
      const models = Array.isArray(params.models) ? params.models : undefined;
      const applied = await applyWorkflowRunSettings(
        {
          runId: String(params.runId),
          // 三态原样透传：键在场即用户改过，`null` 是回到默认。
          ...(params.subagentModel === undefined ? {} : { subagentModel: params.subagentModel }),
          ...(params.maxConcurrency === undefined
            ? {}
            : { maxConcurrency: params.maxConcurrency }),
        },
        {
          sessionId: session,
          workingDirectory: typeof params.cwd === "string" ? params.cwd : process.cwd(),
          ...(models === undefined ? {} : { modelCatalogPort: { listModels: () => models } }),
          port: deps.port(session),
          traceContext: { traceId: `${SETTINGS_TRACE_PREFIX}${session}` } as never,
        },
      );
      if (!applied.ok) return applied as unknown as Record<string, unknown>;
      if (applied.registration !== undefined) {
        stash(applied.toolCallId, applied.registration.backgroundTaskId, applied.registration.toolCall);
      }
      return {
        ok: true,
        runId: applied.runId,
        toolCallId: applied.toolCallId,
        ...(applied.supersededRunId === undefined
          ? {}
          : { supersededRunId: applied.supersededRunId }),
        // 就地调并发没有第二个后台任务要登记，Rust 据此跳过 `run.track`。
        track: applied.registration !== undefined,
        turn: applied.turn,
      };
    },
    trackSaved: async (session: string, toolCallId: string): Promise<Record<string, unknown>> => {
      const pendingRun = pending.get(toolCallId);
      if (pendingRun === undefined) return { ok: false, reason: UNKNOWN_TOOL_CALL };
      pending.delete(toolCallId);
      await deps.track(session, pendingRun.toolCall, pendingRun.runId);
      return { ok: true };
    },
  };
}
