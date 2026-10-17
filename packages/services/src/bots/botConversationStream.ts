// Bot 本地的 V4 conversation 帧投影（docs/specs/bots-v4-stream.md）：只产出 Bot 需要的流信号，
// 不是第二套通用流协议。状态只记录“已经交给 Bot 的进度”，事实仍由 CLI runtime 的快照与增量拥有。
import type {
  ESCodeInteractionRequestOrigin,
  ESCodePermissionOption,
  ESCodeStreamEvent,
  InputId,
  TraceId,
} from "@escode/shared";
import type { Event, IDisposable } from "@escode/rpc";
import type {
  ConversationRow,
  ConversationTopicFrame,
  PendingInteraction,
} from "@escode/shared/escode-protocol-v4";

type RowState = { kind: string; turnId: string; text: string; status?: string; state?: string };
type InteractionKind = PendingInteraction["kind"];

const TOOL_STATUS: Record<string, "pending" | "in_progress" | "completed" | "failed" | "stopped"> =
  {
    inputStreaming: "pending",
    pendingApproval: "pending",
    running: "in_progress",
    success: "completed",
    error: "failed",
    cancelled: "stopped",
  };

// V4 kind 是驼峰值；Bot 的选项分类按文本识别 allow/reject/always，转成 ACP 风格保持原分类结果。
const OPTION_KIND: Record<string, { kind: string; decision: "allow" | "deny" }> = {
  allowOnce: { kind: "allow_once", decision: "allow" },
  allowAlways: { kind: "allow_always", decision: "allow" },
  deny: { kind: "reject_once", decision: "deny" },
  custom: { kind: "custom", decision: "allow" },
};

export interface BotConversationStream {
  /** 接收一帧装配后的 V4 conversation 帧，返回按序要交给 Bot 的流事件。 */
  accept(frame: ConversationTopicFrame): ESCodeStreamEvent[];
}

export function createBotConversationStream(taskId: string): BotConversationStream {
  const rows = new Map<number, RowState>();
  const turnInputs = new Map<string, string>();
  const turnOrigins = new Map<string, "desktop" | "mobile">();
  const interactions = new Map<string, InteractionKind>();
  // 排队项最后一次已知状态（按 sourceCommandId）与已成为本会话输入的 sourceCommandId。
  const queued = new Map<string, { dispatch?: string; steer?: string }>();
  const executed = new Set<string>();
  let baselined = false;
  let terminals: ESCodeStreamEvent[] = [];
  let lastError: string | undefined;

  const identity = (turnId: string | undefined) => {
    const inputId = turnId ? turnInputs.get(turnId) : undefined;
    return {
      taskId,
      traceId: (inputId ?? taskId) as TraceId,
      ...(inputId ? { inputId: inputId as InputId } : {}),
    };
  };

  const textOf = (row: ConversationRow): string =>
    row.kind === "assistantText" || row.kind === "reasoning" ? row.text : "";

  const chunk = (row: { kind: string; turnId: string }, content: string): ESCodeStreamEvent[] => {
    if (!content) return [];
    if (row.kind === "assistantText")
      return [{ type: "agent_message_chunk", ...identity(row.turnId), content }];
    if (row.kind === "reasoning")
      return [{ type: "agent_thought_chunk", ...identity(row.turnId), content }];
    return [];
  };

  const toolEvent = (row: Extract<ConversationRow, { kind: "toolCall" }>, first: boolean) => {
    const status = TOOL_STATUS[row.status] ?? "pending";
    const title = row.toolName;
    const input = row.input ?? parseInput(row.inputText);
    if (first) {
      const call: ESCodeStreamEvent = {
        type: "tool_call",
        ...identity(row.turnId),
        toolId: row.toolCallId,
        parentToolUseId: null,
        input,
        toolName: row.toolName,
        kind: row.toolName,
        title,
        raw: row,
      };
      // 首帧就已越过 pending 的工具（如 snapshot 后才见到）补一次状态，Bot 的工具卡才能收尾。
      return status === "pending" ? [call] : [call, update()];
    }
    return [update()];
    function update(): ESCodeStreamEvent {
      return {
        type: "tool_call_update",
        ...identity(row.turnId),
        toolId: row.toolCallId,
        parentToolUseId: null,
        status,
        title,
        toolName: row.toolName,
        kind: row.toolName,
        input,
        ...(row.output !== undefined ? { content: row.output.text ?? row.output } : {}),
        ...(row.error ? { error: row.error.message } : {}),
        raw: row,
      };
    }
  };

  const onRow = (row: ConversationRow, emit: boolean): ESCodeStreamEvent[] => {
    const previous = rows.get(row.rowId);
    const text = textOf(row);
    const next: RowState = { kind: row.kind, turnId: row.turnId, text };
    const out: ESCodeStreamEvent[] = [];
    if (row.kind === "turnHeader") {
      next.state = row.state;
      if (row.sourceCommandId) {
        turnInputs.set(row.turnId, row.sourceCommandId);
        executed.add(row.sourceCommandId);
      }
      if (emit && row.executionKind !== "controlOnly") {
        const was = previous?.state;
        if (row.state === "running" && was !== "running") {
          const origin = turnOrigins.get(row.turnId);
          out.push({
            type: "task_run_started",
            ...identity(row.turnId),
            turnId: row.turnId,
            startedAt: row.startedAt,
            ...(origin ? { inputOrigin: origin } : {}),
          });
        } else if (was === "running" && row.state !== "running") {
          // 只收口见过 running 的轮：gap 期间开始又结束的旧轮不能当作当前 run 收口。
          // 终态延后到本帧末尾：快照按行序处理，轮头排在正文之前，先收口会丢掉尾部正文。
          terminals.push(...terminal(row.turnId, row.state));
        }
      }
    } else if (row.kind === "userInput") {
      if (row.inputOrigin) turnOrigins.set(row.turnId, row.inputOrigin);
      if (row.sourceCommandId) executed.add(row.sourceCommandId);
    } else if (row.kind === "toolCall") {
      next.status = row.status;
      if (emit && (!previous || previous.status !== row.status))
        out.push(...toolEvent(row, !previous));
    } else if (emit && (row.kind === "assistantText" || row.kind === "reasoning")) {
      // 新文本以已交付文本为前缀时只发后缀；否则（行被重写）按新内容整段交付。
      const sent = previous?.text ?? "";
      out.push(...chunk(next, text.startsWith(sent) ? text.slice(sent.length) : text));
    }
    rows.set(row.rowId, next);
    return out;
  };

  const terminal = (turnId: string, state: string): ESCodeStreamEvent[] => {
    if (state === "completedSuccess")
      return [{ type: "task_complete", ...identity(turnId), stopReason: "end_turn" }];
    if (state === "completedInterrupted")
      return [{ type: "task_complete", ...identity(turnId), stopReason: "cancelled" }];
    if (state === "failed")
      return [{ type: "task_error", ...identity(turnId), error: lastError ?? "Task failed" }];
    return [];
  };

  // 排队项消失：已成为输入（同帧或更早出现轮头/userInput 行）或已进入提升/引导的是被执行；
  // 其余是被删除（增量）或随 runtime 重启丢失（重订阅快照）。
  const onQueue = (
    items: readonly {
      sourceCommandId: string;
      dispatch?: { state?: string };
      steer?: { state?: string };
    }[],
    emit: boolean,
    reason: "queue_deleted" | "session_resumed",
  ): ESCodeStreamEvent[] => {
    const present = new Set(items.map((item) => item.sourceCommandId));
    const out: ESCodeStreamEvent[] = [];
    for (const [id, last] of queued) {
      if (present.has(id)) continue;
      queued.delete(id);
      const started =
        executed.has(id) ||
        ["reserved", "promoting", "drained"].includes(last.dispatch ?? "") ||
        ["steering", "guided"].includes(last.steer ?? "");
      if (emit && !started)
        out.push({
          type: "turn_steer_status",
          taskId,
          traceId: taskId as TraceId,
          status: "discarded",
          pendingInputIds: [id],
          reason,
        });
    }
    for (const item of items)
      queued.set(item.sourceCommandId, {
        dispatch: item.dispatch?.state,
        steer: item.steer?.state,
      });
    return out;
  };

  const onInteractions = (list: readonly PendingInteraction[], emit: boolean) => {
    const out: ESCodeStreamEvent[] = [];
    const present = new Set(list.map((item) => item.interactionId));
    for (const [id, kind] of interactions) {
      if (present.has(id)) continue;
      interactions.delete(id);
      // Bot 只用 elicitation_response 的 requestId 清理待答表单；V4 不携带回答动作，
      // 消失即已解决（回答、超时或轮次结束），统一记为 accept。权限消失 Bot 不消费，不投影。
      if (emit && kind === "userInput")
        out.push({
          type: "elicitation_response",
          ...identity(undefined),
          requestId: id,
          action: "accept",
        });
    }
    for (const item of list) {
      if (interactions.has(item.interactionId)) continue;
      interactions.set(item.interactionId, item.kind);
      if (emit) out.push(...interactionEvent(item));
    }
    return out;
  };

  const interactionEvent = (item: PendingInteraction): ESCodeStreamEvent[] => {
    const anchorTurn = item.anchorRowId !== null ? rows.get(item.anchorRowId)?.turnId : undefined;
    const base = { ...identity(anchorTurn), requestId: item.interactionId };
    const payload = item.payload;
    if (payload.kind === "permission") {
      const options: ESCodePermissionOption[] = payload.options.map((option) => {
        const mapped = OPTION_KIND[option.kind] ?? OPTION_KIND.custom!;
        return {
          optionId: option.optionId,
          kind: mapped.kind,
          name: option.label,
          response: option.response ?? { decision: mapped.decision },
        };
      });
      return [
        {
          type: "permission_request",
          ...base,
          description: payload.summary || payload.toolName,
          kind: payload.toolName,
          title: payload.toolName,
          options,
          ...(payload.freeText ? { freeText: true } : {}),
          ...(payload.origin ? { origin: payload.origin as ESCodeInteractionRequestOrigin } : {}),
          ...(payload.display ? { display: payload.display } : {}),
          raw: payload,
        },
      ];
    }
    if (payload.kind === "userInput") {
      const questions = (payload.questions ?? []).map((question) => ({
        question: question.question,
        header: question.header,
        options: question.options.map((option) => ({
          value: option.value,
          label: option.label,
          description: option.description,
        })),
        ...(question.multiSelect ? { multiSelect: true } : {}),
      }));
      const first = questions[0];
      const fallbackOptions = (payload.options ?? []).map((option) => ({
        value: option.optionId,
        label: option.label,
      }));
      return [
        {
          type: "elicitation_request",
          ...base,
          message: first?.question ?? payload.prompt ?? "Input required",
          header: first?.header,
          options: first?.options ?? fallbackOptions,
          ...(first?.multiSelect ? { multiSelect: true } : {}),
          ...(questions.length > 0 ? { questions } : {}),
          ...(payload.currentQuestionIndex !== undefined
            ? { currentQuestionIndex: payload.currentQuestionIndex }
            : {}),
          ...(payload.answerDrafts ? { answerDrafts: payload.answerDrafts } : {}),
          ...(payload.origin ? { origin: payload.origin as ESCodeInteractionRequestOrigin } : {}),
          schema: payload.schema ?? payload.input,
        },
      ];
    }
    return [];
  };

  return {
    accept(frame) {
      terminals = [];
      const out = acceptFrame(frame);
      return [...out, ...terminals];
    },
  };

  function acceptFrame(frame: ConversationTopicFrame): ESCodeStreamEvent[] {
    const out: ESCodeStreamEvent[] = [];
    if (frame.payload.kind === "snapshot") {
      const snapshot = frame.payload.snapshot;
      // 首帧只建基线（continuous 不重放旧轮）；gap 后的重订阅快照按已交付进度续发。
      const emit = baselined;
      lastError = snapshot.control.lastError?.message;
      for (const row of snapshot.rows.window) out.push(...onRow(row, emit));
      out.push(...onInteractions(snapshot.pendingInteractions, emit));
      out.push(...onQueue(snapshot.queue?.items ?? [], emit, "session_resumed"));
      baselined = true;
      return out;
    }
    // 同批里的 lastError 可能排在终态行之后；先取出，task_error 才带得上真实原因。
    for (const delta of frame.payload.deltas) {
      if (delta.op === "state.updated" && delta.patch.control)
        lastError = delta.patch.control.lastError?.message;
      // 轮头先于 userInput 行到达；run_started 要带的输入来源从同批 userInput 行取。
      if (
        (delta.op === "row.appended" || delta.op === "row.upserted") &&
        delta.row.kind === "userInput" &&
        delta.row.inputOrigin
      )
        turnOrigins.set(delta.row.turnId, delta.row.inputOrigin);
    }
    let queueItems: Parameters<typeof onQueue>[0] | undefined;
    for (const delta of frame.payload.deltas) {
      if (delta.op === "state.updated" && delta.patch.queue) queueItems = delta.patch.queue.items;
      if (delta.op === "row.appended" || delta.op === "row.upserted") {
        out.push(...onRow(delta.row, true));
      } else if (delta.op === "row.delta") {
        const row = rows.get(delta.rowId);
        if (!row || delta.path !== "text") continue;
        row.text += delta.append;
        out.push(...chunk(row, delta.append));
      } else if (delta.op === "row.removed") {
        for (const rowId of rows.keys()) if (rowId >= delta.fromRowId) rows.delete(rowId);
      } else if (delta.op === "state.updated" && delta.patch.pendingInteractions) {
        out.push(...onInteractions(delta.patch.pendingInteractions, true));
      }
    }
    // 队列在本帧所有行之后判定：提升的轮头与队列补丁常在同一帧，顺序不保证。
    if (queueItems) out.push(...onQueue(queueItems, true, "queue_deleted"));
    return out;
  }
}

function parseInput(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

/** 任务流上游：订阅 V4 帧并经同一个投影实例按序交付（一个上游对应一个投影，进度不跨订阅共享）。 */
export function subscribeBotConversationStream(
  frames: (params: {
    workspacePath: string;
    workspaceIdentity?: string;
    taskId: string;
  }) => Event<ConversationTopicFrame>,
  target: { workspacePath: string; workspaceIdentity?: string; taskId: string },
  receive: (event: ESCodeStreamEvent) => Promise<void>,
): IDisposable {
  const projection = createBotConversationStream(target.taskId);
  return frames(target)((frame) => {
    for (const event of projection.accept(frame)) void receive(event);
  });
}
