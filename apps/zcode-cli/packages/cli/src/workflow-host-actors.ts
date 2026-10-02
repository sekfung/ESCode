/**
 * 工作流 actor 的远程 AgentRuntime（docs/specs/rust-dynamic-workflow.md「M2 设计」）。
 *
 * TS driver 只用到 actor runtime 的窄面：`executeTurn` / `subscribeEvents` / 工具注册表 / `close`。
 * 这里给出同一个面，但每一轮都交给 Rust 的 actor 子会话执行；`submit_result` / `escalate` 这两个
 * 由 driver 裁决的工具留在宿主：Rust 遇到它们的调用时回调 `actor.tool`，这里跑 TS 原版 handler
 * （阻塞在 driver 的 deferred 上），把结果交回 Rust。actor 会话的运行事件经 `actor.event` 回来，
 * 原样分发给 driver 的订阅者。
 */

import { CoreErrorType } from "@zcode/contracts";
import { workflowActorModelPolicy, workflowActorToolPolicy } from "@zcode/bootstrap";
import {
  buildWorkflowActorIdentityPrompt,
  builtInTools,
  createSubmitResultToolEntry,
  submitResultToolEntry,
} from "@zcode/core";

export type RustRequest = (method: string, params: Record<string, unknown>) => Promise<any>;

type ToolEntry = (typeof builtInTools)[number];
type Listener = (event: unknown) => void;

interface ActorState {
  input: Record<string, any>;
  entries: Map<string, ToolEntry>;
  listeners: Set<Listener>;
}

const toolDefinition = (entry: ToolEntry) => ({
  name: entry.metadata.name,
  description: entry.metadata.description,
  parameters: entry.inputSchema,
});

export function createActorBridge(
  rustRequest: RustRequest,
  parentSession: string,
  /** 父会话当前的模型选择（Rust 随每次工作流工具调用带来）。 */
  parentSelection: () => Record<string, unknown> | undefined,
) {
  const actors = new Map<string, ActorState>();
  const escalateEntry = builtInTools.find((tool) => tool.metadata.name === "escalate");

  const remoteTools = (state: ActorState) => [...state.entries.values()].map(toolDefinition);

  /** run 服务的 `createActorRuntime`：返回远程 runtime（driver 只用到它的这几个成员）。 */
  const createActorRuntime = (input: Record<string, any>): never => {
    const profile = input.submitProfile as { kind: string; schema?: unknown };
    const entries = new Map<string, ToolEntry>();
    if (profile.kind === "mono") {
      const typed = createSubmitResultToolEntry(profile.schema as never);
      entries.set(typed.metadata.name, typed);
    } else if (profile.kind === "generic") {
      entries.set(submitResultToolEntry.metadata.name, submitResultToolEntry);
    }
    if (escalateEntry !== undefined) entries.set(escalateEntry.metadata.name, escalateEntry);
    const state: ActorState = { input, entries, listeners: new Set() };
    const actorSession = String(input.sessionId);
    actors.set(actorSession, state);
    const persona = input.persona as { name?: string; system?: string };
    // TS workflowActorModelPolicy：run 的 subagent_model > pin > 父会话当前选择。driver 在造好 runtime 后
    // 同步读 getSessionModelSelection 记 resolvedModel，所以选择在宿主里算好，再交给 Rust 使用。
    const parent = parentSelection();
    const policy = workflowActorModelPolicy(
      {
        ...(parent === undefined ? {} : { parentSelection: parent as never }),
        ...(input.runSubagentModel === undefined ? {} : { runSelection: input.runSubagentModel }),
      },
      input.pinnedModel,
    );
    const selection = (policy.configOverrides as { modelSelection?: unknown }).modelSelection ?? parent;
    const ready = rustRequest("actor.create", {
      session: parentSession,
      actorSession,
      persona: {
        ...(persona.name === undefined ? {} : { name: persona.name }),
        identityPrompt: buildWorkflowActorIdentityPrompt({
          ...(persona.name === undefined ? {} : { name: persona.name }),
          ...(persona.system === undefined ? {} : { persona: persona.system }),
        }),
      },
      ...(selection === undefined ? {} : { selection }),
      disallowed: workflowActorToolPolicy().toolDisallowlist,
      remoteTools: remoteTools(state),
    });
    const runtime = {
      async executeTurn(text: string, _unused: unknown, options?: Record<string, any>) {
        await ready;
        const signal = options?.abortSignal as AbortSignal | undefined;
        const cancel = () => void rustRequest("actor.cancel", { actorSession }).catch(() => {});
        signal?.addEventListener("abort", cancel, { once: true });
        try {
          const result = await rustRequest("actor.turn", {
            actorSession,
            input: text,
            ...(options?.epilogueStart === undefined ? {} : { epilogueStart: options.epilogueStart }),
          });
          if (result.cancelled === true || signal?.aborted === true) {
            throw Object.assign(new Error("Turn cancelled"), { type: CoreErrorType.TurnCancelled });
          }
          if (result.error !== undefined) {
            const error = new Error(String(result.error.message ?? result.error));
            // 模型失败带分类时还原成 TS adapter 错误的形状（name / code / context），
            // driver 的 inspectWorkflowModelFailure 据此决定停 run、重驱或 ContextLimit。
            if (result.error.context !== undefined) {
              const context = Object.fromEntries(
                Object.entries(result.error.context as Record<string, unknown>).filter(([, v]) => v != null),
              );
              Object.assign(error, { name: "AiSdkModelAdapterError", code: result.error.code, context });
            }
            throw error;
          }
          return { response: result.response ?? "", usage: result.usage };
        } finally {
          signal?.removeEventListener("abort", cancel);
        }
      },
      subscribeEvents(listener: Listener) {
        state.listeners.add(listener);
        return () => {
          state.listeners.delete(listener);
        };
      },
      getToolRegistry() {
        return {
          register(entry: ToolEntry) {
            state.entries.set(entry.metadata.name, entry);
            void rustRequest("actor.tools", { actorSession, remoteTools: remoteTools(state) });
          },
        };
      },
      invalidateToolCache() {},
      /** TS persistActorSession：落会话行并定标题（`workflow subagent <ref>`）。 */
      async ensureSessionPersistedForExternalActivity(title: string) {
        await ready;
        await rustRequest("actor.title", { actorSession, title });
      },
      getMode() {
        return "yolo";
      },
      updateConfig() {},
      getSessionModelSelection() {
        return selection;
      },
      /** resume 重水化：Rust 从会话库重载该 actor 会话（被打断的工具调用按 TS hydrator 语义补齐）。 */
      async resumeFromStore() {
        await ready;
        const result = await rustRequest("actor.resume", { actorSession });
        if (result?.found !== true) {
          throw Object.assign(new Error(`Session not found: ${actorSession}`), {
            type: CoreErrorType.SessionNotFound,
          });
        }
      },
      async closeBrowserSession() {},
      async close() {
        actors.delete(actorSession);
        await rustRequest("actor.close", { actorSession }).catch(() => {});
      },
    };
    return runtime as never;
  };

  /** Rust 回调：在 actor 会话里执行宿主侧工具（submit_result / escalate）。 */
  const handleTool = async (params: Record<string, any>) => {
    const state = actors.get(params.actorSession);
    const entry = state?.entries.get(params.tool);
    if (state === undefined || entry === undefined) {
      return { content: `Unknown workflow actor tool: ${params.tool}`, isError: true };
    }
    try {
      const output = await entry.handler(params.input, {
        toolCallId: params.callId,
        sessionId: params.actorSession,
        workflowSubmitPort: state.input.submitPort,
        workflowEscalatePort: state.input.escalatePort,
      } as never);
      const failure = output as { result?: unknown; message?: unknown };
      if (failure !== null && typeof failure === "object" && failure.result === false) {
        return { content: String(failure.message ?? ""), isError: true, handlerFailure: true };
      }
      const content = entry.formatModelContent
        ? entry.formatModelContent(output)
        : JSON.stringify(output);
      return {
        content,
        isError: false,
        stopTurn: (entry.metadata as { stopTurnOnSuccess?: boolean }).stopTurnOnSuccess === true,
      };
    } catch (error) {
      return { content: error instanceof Error ? error.message : String(error), isError: true };
    }
  };

  /**
   * Rust 通知：actor 会话的简化运行事件，合成 TS SessionEvent 交给 driver 的订阅者。
   * - `{kind: "toolStart", callId, name, input, readOnly?}` → ToolCallScheduled + ToolCallStarted
   *   （副作用能力取 TS 工具元数据；Bash 的只读判定由 Rust 按命令给出）；
   * - `{kind: "toolEnd", callId, error}` → ToolCallResult / ToolCallError；
   * - `{kind: "model", type, requestId, ...}` → ModelNetworkStatus（`model_request_started` 等）。
   */
  const handleEvent = (params: Record<string, any>) => {
    const state = actors.get(params.actorSession);
    if (state === undefined) return;
    const sessionId = params.actorSession as string;
    const event = params.event as Record<string, any>;
    const emit = (type: string, payload: unknown) => {
      for (const listener of state.listeners) {
        const target = listener as unknown as { onSessionEvent?: (event: unknown) => void };
        const sessionEvent = { type, sessionId, timestamp: new Date(), payload };
        if (typeof target.onSessionEvent === "function") target.onSessionEvent(sessionEvent);
        else if (typeof listener === "function") listener(sessionEvent);
      }
    };
    if (event.kind === "toolStart") {
      const metadata = builtInTools.find((tool) => tool.metadata.name === event.name)?.metadata as
        | { readOnly?: boolean; sideEffectScope?: string }
        | undefined;
      emit("tool_call_scheduled", {
        toolCallId: event.callId,
        toolName: event.name,
        input: event.input,
        schedule: {},
      });
      emit("tool_call_started", {
        toolCallId: event.callId,
        toolName: event.name,
        startedAt: new Date(),
        readOnly: typeof event.readOnly === "boolean" ? event.readOnly : metadata?.readOnly,
        ...(metadata?.sideEffectScope === undefined ? {} : { sideEffectScope: metadata.sideEffectScope }),
      });
    } else if (event.kind === "toolEnd") {
      emit(event.error === true ? "tool_call_error" : "tool_call_result", { toolCallId: event.callId });
    } else if (event.kind === "model") {
      const { kind: _kind, ...status } = event;
      emit("model_network_status", status);
    }
  };

  return { createActorRuntime, handleTool, handleEvent };
}
