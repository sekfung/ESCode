import { resolveExecutionState, type ExecutionState } from "@escode/shared";
import {
  SESSION_ENTRY_EXECUTION_STATE,
  SessionEventType,
  type TraceContext,
  type SessionId,
  type SessionEntryInfo,
} from "@escode/contracts";
import type { AgentRuntimeInternal } from "./internal.js";
import {
  unpublishedPermissionGrants,
  recoverPendingPermissionGrant,
} from "./permission-grant-recovery.js";

export function readRuntimeExecutionState(runtime: AgentRuntimeInternal): ExecutionState {
  return resolveExecutionState(runtime.config);
}

async function persistExecutionState(
  runtime: AgentRuntimeInternal,
  state = readRuntimeExecutionState(runtime),
): Promise<void> {
  if (!runtime.sessionPersisted || !runtime.sessionStore?.saveSessionEntry) return;
  await runtime.sessionStore.saveSessionEntry(buildExecutionStateEntry(runtime.sessionId, state));
}

export function buildExecutionStateEntry(
  sessionId: SessionId,
  state: ExecutionState,
): SessionEntryInfo {
  const timestamp = Date.now();
  return {
    id: `${sessionId}:runtime-execution-state`,
    sessionID: sessionId,
    type: SESSION_ENTRY_EXECUTION_STATE,
    touchSession: false,
    time: { created: timestamp, updated: timestamp },
    data: state,
  };
}

/** 权限与 Plan 是一个已消费状态；保存失败不发布成功快照，也不提前改内存。 */
export async function applyRuntimeExecutionState(
  runtime: AgentRuntimeInternal,
  input: { mode?: string; planEnabled?: boolean },
  cause: { source: "command" | "tool"; toolCallId?: string; traceContext?: TraceContext },
): Promise<ExecutionState> {
  if (runtime.permissionFullAccessPending)
    throw new Error("Permission update is busy; retry mode change");
  if (unpublishedPermissionGrants.has(runtime)) await recoverPendingPermissionGrant(runtime);
  const previous = readRuntimeExecutionState(runtime);
  const next = resolveExecutionState(input, previous);
  if (next.mode === previous.mode && next.planEnabled === previous.planEnabled) return next;
  if (next.planEnabled && !previous.planEnabled) {
    const goal = await runtime.readSessionTargetForContext?.(
      cause.traceContext ?? runtime.rootTraceContext,
    );
    if (goal?.status === "active")
      throw new Error("Plan and Goal cannot be active at the same time.");
  }
  await persistExecutionState(runtime, next);
  runtime.config.mode = next.mode;
  runtime.config.planEnabled = next.planEnabled;
  if (previous.planEnabled !== next.planEnabled)
    runtime.needsPlanModeExitReminder = !next.planEnabled;
  const trace = cause.traceContext ?? runtime.rootTraceContext;
  await runtime.appendEvent(
    runtime.createEvent(
      SessionEventType.SessionModeChanged,
      {
        ...next,
        previousMode: previous.mode,
        previousPlanEnabled: previous.planEnabled,
        source: cause.source,
        ...(cause.toolCallId ? { toolCallId: cause.toolCallId } : {}),
      },
      trace,
    ),
    trace,
  );
  return next;
}

/**
 * 冷恢复后让事件投影与权威执行状态对齐。
 *
 * 修复：resume 按 execution-state entry 恢复模式与 Plan 时只写 config、不进事件流；事件流里没有模式事件的会话
 * （如 fork child：entry 取被选轮的模式，父会话事件不复制）投影回落默认 build，session/read 的 session.mode 与
 * settings.permission.mode、实际执行互相矛盾。与 applyRuntimeExecutionState 一样补一条 SessionModeChanged，
 * 让投影与执行状态是同一事实（docs/specs/rust-v4-command-gaps.md「forkAssistant」）。一致时不产事件。
 */
export async function reconcileProjectedExecutionState(
  runtime: AgentRuntimeInternal,
  traceContext: TraceContext,
): Promise<void> {
  const projected = await runtime.rebuildProjection();
  const current = readRuntimeExecutionState(runtime);
  if (
    projected.mode === current.mode &&
    (projected.planEnabled ?? false) === (current.planEnabled ?? false)
  )
    return;
  await runtime.appendEvent(
    runtime.createEvent(
      SessionEventType.SessionModeChanged,
      {
        ...current,
        previousMode: projected.mode,
        previousPlanEnabled: projected.planEnabled,
        source: "system",
      },
      traceContext,
    ),
    traceContext,
  );
}
