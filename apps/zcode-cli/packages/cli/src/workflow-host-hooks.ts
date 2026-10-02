/**
 * Rust runtime 的 hooks 面（docs/specs/rust-hooks.md H1）：每个会话按 Node 启动同一口径装配 hook 运行器
 * （bootstrap `resolveRuntimeHooks` + core `createConfiguredHookRunner`），执行由 Rust 在各调用点发起；
 * 生命周期事件（HookRun*）实时通知 Rust 投影成 `hookInvocation` 行。
 */

import { resolveRuntimeHooks } from "@zcode/bootstrap";
import { createConfiguredHookRunner, hookMatcherToolNamesForTool } from "@zcode/core";

type Send = (message: unknown) => void;
type Runner = ReturnType<typeof createConfiguredHookRunner>;

const TOOL_EVENTS = new Set(["PreToolUse", "PermissionRequest", "PostToolUse", "PostToolUseFailure"]);

/** TS tool/executor/utils previewHookValue。 */
function previewHookValue(value: unknown): string {
  try {
    const serialized = typeof value === "string" ? value : JSON.stringify(value);
    if (!serialized) return "";
    return serialized.length <= 4000 ? serialized : `${serialized.slice(0, 4000)}...[truncated]`;
  } catch {
    return String(value);
  }
}

export function createHookHost(send: Send, executionPort: unknown) {
  const runners = new Map<string, { cwd: string; runner: Runner }>();
  const runnerFor = (session: string, cwd: string): Runner => {
    const existing = runners.get(session);
    if (existing && existing.cwd === cwd) return existing.runner;
    const config = resolveRuntimeHooks({ workingDirectory: cwd, env: process.env });
    const runner = config
      ? createConfiguredHookRunner({
          config,
          executionPort: executionPort as never,
          getWorkingDirectory: () => cwd,
          emitEvent: async (event) => {
            send({
              event: "hookEvent",
              params: { session, kind: "hookEvent", at: new Date(event.timestamp).getTime(), event },
            });
          },
        })
      : undefined;
    runners.set(session, { cwd, runner });
    return runner;
  };

  /** `hooks.run {session, cwd, input}` → TS HookRunResult（无运行器时为空结果）。 */
  const run = async (params: Record<string, any>, signal: AbortSignal) => {
    const runner = runnerFor(params.session, params.cwd);
    if (!runner) return { additionalContexts: [], configured: false };
    const input = params.input as Record<string, any>;
    input.cwd ??= params.cwd;
    input.timestamp ??= new Date().toISOString();
    for (const key of Object.keys(input)) if (input[key] === null) delete input[key];
    const event = String(input.hookEventName);
    if (event === "PostToolUse") input.toolResultPreview ??= previewHookValue(input.toolResponse);
    const options = TOOL_EVENTS.has(event)
      ? {
          matchValue: input.toolName,
          matchValues: hookMatcherToolNamesForTool(String(input.toolName)),
          signal,
        }
      : event === "SessionStart"
        ? { matchValue: input.source, signal }
        : { signal };
    return { ...(await runner.run(input as never, options)), configured: true };
  };

  const close = (session: string) => {
    runners.delete(session);
  };

  return { run, close };
}
