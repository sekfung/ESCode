/**
 * Rust runtime 的 hooks 面（docs/specs/rust-hooks.md）：每个会话按 Node 启动同一口径装配 hook 运行器
 * （bootstrap `resolveRuntimeHookContext` + core `createConfiguredHookRunner`），执行由 Rust 在各调用点发起；
 * 生命周期事件（HookRun*）实时通知 Rust 投影成 `hookInvocation` 行。
 *
 * H2：工作区（项目）hooks 的信任准入与审核复用 bootstrap `createWorkspaceHookRuntimeSecurity`（与 app-server
 * 同一份配置：trust 开启、宿主级 policy provider、审核宿主上下文）；准入 / 审核事件同样经 `hookEvent` 通知 Rust。
 *
 * H3：`ZCODE_MESSAGE_ENABLED` 灰度下注册会话 mailbox 内部 hooks（TS runtime-tools）；PostToolUse 取到的消息经
 * `mailboxGuide` 通知 Rust 作为本轮 guide 输入（TS steerTurn delivery guide）。
 */

import { basename } from "node:path";
import { homedir } from "node:os";
import { createNodeSessionMailboxAdapter } from "@zcode/adapters/mailbox";
import { randomUUID } from "node:crypto";
import {
  createWorkspaceHookRuntimeSecurity,
  grantWorkspaceHookTrustForProtocol,
  resolveRuntimeHookContext,
} from "@zcode/bootstrap";
import {
  InMemoryWorkspaceHookPolicyProvider,
  createConfiguredHookRunner,
  createInMemoryHookRunner,
  createSessionMailboxHookRegistrations,
  hookMatcherToolNamesForTool,
} from "@zcode/core";

type Send = (message: unknown) => void;
type Runner = ReturnType<typeof createConfiguredHookRunner>;
type Security = ReturnType<typeof createWorkspaceHookRuntimeSecurity>;

const TOOL_EVENTS = new Set(["PreToolUse", "PermissionRequest", "PostToolUse", "PostToolUseFailure"]);

const SILENT_LOGGER = {
  child: () => SILENT_LOGGER,
  debug: () => {},
  error: () => {},
  info: () => {},
  warn: () => {},
} as any;

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

/** 审核命令的 flow 目标（TS create-app 的同名映射）。 */
function reviewTarget(input: Record<string, any>) {
  return {
    sessionId: input.sessionId,
    taskId: input.taskId,
    runId: input.runId,
    ...(input.remoteSessionId ? { remoteSessionId: input.remoteSessionId } : {}),
    workspaceIdentity: input.workspaceIdentity,
    bundleDigest: input.bundleDigest,
    reviewFlowId: input.reviewFlowId,
    generation: input.generation,
    interactionId: input.interactionId,
  };
}

/** TS bootstrap isMessageEnabled + ZCODE_MAILBOX_ROOT（默认 ~/.zcode/mailbox）。 */
function mailboxPort() {
  const flag = process.env.ZCODE_MESSAGE_ENABLED;
  if (flag !== "1" && flag !== "true") return undefined;
  const root = (process.env.ZCODE_MAILBOX_ROOT ?? "~/.zcode/mailbox").replace(/^~(?=$|[\/])/u, homedir());
  return createNodeSessionMailboxAdapter({ rootDir: root });
}

const UNAVAILABLE = { accepted: false, reasonCode: "workspace_hooks_require_trust_capable_host" };

export function createHookHost(send: Send, executionPort: unknown) {
  const policyProvider = new InMemoryWorkspaceHookPolicyProvider();
  const sessions = new Map<string, { cwd: string; runner: Runner; security: Security }>();
  const notify = (session: string, event: { type: string; timestamp?: Date }) => {
    const at = event.timestamp ? new Date(event.timestamp).getTime() : Date.now();
    send({ event: "hookEvent", params: { session, kind: "hookEvent", at, event } });
  };
  const entryFor = (session: string, cwd: string) => {
    const existing = sessions.get(session);
    if (existing && existing.cwd === cwd) return existing;
    const { hooks, configResult } = resolveRuntimeHookContext({ workingDirectory: cwd, env: process.env });
    const security = createWorkspaceHookRuntimeSecurity({
      logger: SILENT_LOGGER,
      policyProvider,
      workspaceHookTrustEnabled: true,
      reviewHost: {
        taskId: session,
        runId: `workspace-hook-run:${session}:${randomUUID()}`,
        workspaceLabel: basename(cwd) || cwd,
      },
      runtimeRoot: configResult.sources.project.workspaceHookRuntimeRoot ?? {
        enabled: hooks?.enabled === true,
        timeoutMs: hooks?.timeoutMs ?? 60_000,
        maxOutputBytes: hooks?.maxOutputBytes ?? 32_768,
      },
      sessionId: session as never,
      snapshot: configResult.sources.project.workspaceHookSnapshot,
      userConfigPath: configResult.sources.user.path,
      workingDirectory: cwd,
      emitReviewEvent: async (event) => notify(session, event),
      emitAdmissionEvent: async (event) => notify(session, event),
    });
    let runner: Runner = hooks?.enabled || security
      ? createConfiguredHookRunner({
          config: hooks ?? { enabled: false, events: {}, timeoutMs: 60_000, maxOutputBytes: 32_768 },
          executionPort: executionPort as never,
          getWorkingDirectory: () => cwd,
          ...(security
            ? { workspaceHookAdmission: security.admission, workspaceHookSnapshot: security.snapshot }
            : {}),
          emitEvent: async (event) => notify(session, event),
        })
      : undefined;
    const mailbox = mailboxPort();
    if (mailbox) {
      runner ??= createInMemoryHookRunner({ emitEvent: async (event) => notify(session, event) });
      for (const hook of createSessionMailboxHookRegistrations({
        enqueuePendingInput: async (text, trace) => {
          send({ event: "hookEvent", params: { session, kind: "mailboxGuide", text, turnId: trace.turnId } });
        },
        mailbox,
        sessionId: session as never,
      })) {
        if (runner && "register" in runner && typeof runner.register === "function") runner.register(hook);
      }
    }
    const entry = { cwd, runner, security };
    sessions.set(session, entry);
    return entry;
  };

  /** `hooks.run {session, cwd, input}` → TS HookRunResult（无运行器且无工作区 hooks 时 `configured: false`）。 */
  const run = async (params: Record<string, any>, signal: AbortSignal) => {
    const { runner, security } = entryFor(params.session, params.cwd);
    const input = params.input as Record<string, any>;
    const event = String(input.hookEventName);
    // TS runSessionStartHooks：工作区准入先于 SessionStart 激活（软门禁上报 pending 状态）。
    if (event === "SessionStart") await security?.admission.activate(input.source, signal);
    if (!runner) return { additionalContexts: [], configured: security !== undefined };
    input.cwd ??= params.cwd;
    input.timestamp ??= new Date().toISOString();
    for (const key of Object.keys(input)) if (input[key] === null) delete input[key];
    if (event === "PostToolUse") input.toolResultPreview ??= previewHookValue(input.toolResponse);
    if (event === "Stop" && typeof input.responseText === "string") {
      input.responsePreview ??=
        input.responseText.length <= 4000 ? input.responseText : `${input.responseText.slice(0, 4000)}...`;
    }
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

  /** `hooks.review {session, cwd, command, payload}`：V4 工作区 hook 审核命令（TS interaction-background）。 */
  const review = async (params: Record<string, any>) => {
    const { security } = entryFor(params.session, params.cwd);
    if (!security) return UNAVAILABLE;
    const payload = params.payload as Record<string, any>;
    switch (params.command) {
      case "respondWorkspaceHookReview":
        return security.respond(reviewTarget(payload) as never, payload.decision);
      case "toggleWorkspaceHookReviewItem": {
        const { request: _request, ...result } = await security.toggle(
          reviewTarget(payload) as never,
          payload.reviewItemId,
          payload.enabled,
        );
        return result;
      }
      case "revokeWorkspaceHookTrust":
        return "hookDeclarationDigests" in payload
          ? security.revokeCurrent(payload as never)
          : security.revoke(reviewTarget(payload) as never, payload.reviewItemIds);
      case "requestWorkspaceHookReview":
        return security.requestReview({
          workspaceIdentity: payload.workspaceIdentity,
          bundleDigest: payload.bundleDigest,
        });
      default:
        return UNAVAILABLE;
    }
  };

  /** `hooks.trustGrant {params}`：无会话的 Settings 授权（TS server workspaceHookTrustGrant），成功后重载同工作区会话。 */
  const trustGrant = async (params: Record<string, any>) => {
    const result = await grantWorkspaceHookTrustForProtocol(params.params, { policyProvider });
    if (result.accepted) {
      const path = params.params?.workspace?.workspacePath;
      await Promise.all(
        [...sessions.values()]
          .filter((entry) => entry.cwd === path)
          .map((entry) => entry.security?.reloadTrust()),
      );
    }
    return result;
  };

  const close = (session: string) => {
    sessions.delete(session);
  };

  return { run, review, trustGrant, close };
}
