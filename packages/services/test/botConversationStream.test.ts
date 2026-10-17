import { describe, expect, it } from "vitest";
import type { ConversationTopicFrame } from "@escode/shared/escode-protocol-v4";
import { createBotConversationStream } from "../src/bots/botConversationStream.js";

// docs/specs/bots-v4-stream.md §4：V4 帧 → Bot 流信号。
const header = (rowId: number, state: string, extra: Record<string, unknown> = {}) => ({
  rowId,
  turnId: "turn-1",
  kind: "turnHeader",
  origin: "userInput",
  sourceCommandId: "input-1",
  state,
  startedAt: 10,
  ...extra,
});
const text = (rowId: number, value: string, kind = "assistantText") => ({
  rowId,
  turnId: "turn-1",
  kind,
  text: value,
  state: "streaming",
  assistantResponseId: "resp-1",
});
const tool = (rowId: number, status: string, extra: Record<string, unknown> = {}) => ({
  rowId,
  turnId: "turn-1",
  kind: "toolCall",
  toolCallId: "call-1",
  toolName: "Read",
  status,
  inputText: '{"file_path":"a.txt"}',
  ...extra,
});
const snapshot = (
  window: unknown[],
  pendingInteractions: unknown[] = [],
  lastError?: string,
): ConversationTopicFrame =>
  ({
    topic: "conversation/task",
    subscriptionId: "s",
    fromSeq: 0,
    toSeq: 1,
    sentAt: 1,
    payload: {
      kind: "snapshot",
      snapshot: {
        control: lastError ? { lastError: { message: lastError } } : {},
        rows: { window },
        pendingInteractions,
      },
    },
  }) as never;
const deltas = (...items: unknown[]): ConversationTopicFrame =>
  ({
    topic: "conversation/task",
    subscriptionId: "s",
    fromSeq: 1,
    toSeq: 2,
    sentAt: 1,
    payload: { kind: "deltas", deltas: items },
  }) as never;
const upsert = (row: unknown) => ({ op: "row.upserted", row });
const append = (row: unknown) => ({ op: "row.appended", row });
const types = (events: { type: string }[]) => events.map((event) => event.type);

describe("bot conversation stream projection", () => {
  it("baselines the first snapshot without replaying old text", () => {
    const stream = createBotConversationStream("task");
    expect(stream.accept(snapshot([header(1, "completedSuccess"), text(2, "old answer")]))).toEqual(
      [],
    );
  });

  it("projects a turn: run started, text suffixes, tool lifecycle, completion", () => {
    const stream = createBotConversationStream("task");
    stream.accept(snapshot([]));
    const started = stream.accept(
      deltas(
        append(header(1, "running")),
        append({
          rowId: 2,
          turnId: "turn-1",
          kind: "userInput",
          text: "hi",
          inputOrigin: "mobile",
        }),
      ),
    );
    expect(started).toMatchObject([
      { type: "task_run_started", inputId: "input-1", traceId: "input-1", inputOrigin: "mobile" },
    ]);
    expect(stream.accept(deltas(append(text(3, "Hel"))))).toMatchObject([
      { type: "agent_message_chunk", content: "Hel", inputId: "input-1" },
    ]);
    // 增量追加与整行替换都只交付新增后缀。
    expect(
      stream.accept(deltas({ op: "row.delta", rowId: 3, path: "text", append: "lo" })),
    ).toMatchObject([{ content: "lo" }]);
    expect(stream.accept(deltas(upsert(text(3, "Hello world"))))).toMatchObject([
      { content: " world" },
    ]);
    expect(stream.accept(deltas(append(text(4, "think", "reasoning"))))).toMatchObject([
      { type: "agent_thought_chunk", content: "think" },
    ]);
    expect(stream.accept(deltas(append(tool(5, "running"))))).toMatchObject([
      { type: "tool_call", toolId: "call-1", toolName: "Read", input: { file_path: "a.txt" } },
      { type: "tool_call_update", status: "in_progress" },
    ]);
    expect(
      stream.accept(deltas(upsert(tool(5, "success", { output: { text: "content" } })))),
    ).toMatchObject([{ type: "tool_call_update", status: "completed", content: "content" }]);
    expect(stream.accept(deltas(upsert(header(1, "completedSuccess"))))).toMatchObject([
      { type: "task_complete", stopReason: "end_turn", inputId: "input-1" },
    ]);
  });

  it("maps interrupted and failed terminals, taking lastError from the same batch", () => {
    const interrupted = createBotConversationStream("task");
    interrupted.accept(snapshot([header(1, "running")]));
    expect(interrupted.accept(deltas(upsert(header(1, "completedInterrupted"))))).toMatchObject([
      { type: "task_complete", stopReason: "cancelled" },
    ]);
    const failed = createBotConversationStream("task");
    failed.accept(snapshot([header(1, "running")]));
    expect(
      failed.accept(
        deltas(upsert(header(1, "failed")), {
          op: "state.updated",
          patch: { control: { lastError: { message: "model unavailable" } } },
        }),
      ),
    ).toMatchObject([{ type: "task_error", error: "model unavailable" }]);
  });

  it("does not announce control-only turns", () => {
    const stream = createBotConversationStream("task");
    stream.accept(snapshot([]));
    expect(
      stream.accept(deltas(append(header(1, "running", { executionKind: "controlOnly" })))),
    ).toEqual([]);
  });

  it("projects permission and question interactions once and their resolution", () => {
    const stream = createBotConversationStream("task");
    stream.accept(snapshot([header(1, "running")]));
    const permission = {
      interactionId: "perm-1",
      kind: "permission",
      anchorRowId: 1,
      createdAt: 1,
      payload: {
        kind: "permission",
        toolCallId: "call-1",
        toolName: "Bash",
        summary: "Run ls",
        detail: null,
        options: [
          { optionId: "allow", label: "Allow", kind: "allowOnce" },
          { optionId: "deny", label: "Deny", kind: "deny" },
        ],
      },
    };
    const question = {
      interactionId: "ask-1",
      kind: "userInput",
      anchorRowId: 1,
      createdAt: 1,
      payload: {
        kind: "userInput",
        prompt: "Pick",
        freeText: false,
        questions: [
          { question: "Which?", header: "Choice", options: [{ value: "a", label: "A" }] },
        ],
      },
    };
    const patch = (list: unknown[]) => ({
      op: "state.updated",
      patch: { pendingInteractions: list },
    });
    expect(stream.accept(deltas(patch([permission, question])))).toMatchObject([
      {
        type: "permission_request",
        requestId: "perm-1",
        inputId: "input-1",
        description: "Run ls",
        options: [
          { optionId: "allow", kind: "allow_once", name: "Allow", response: { decision: "allow" } },
          { optionId: "deny", kind: "reject_once", response: { decision: "deny" } },
        ],
      },
      { type: "elicitation_request", requestId: "ask-1", message: "Which?", header: "Choice" },
    ]);
    // 同一交互重复出现在后续 patch 里不再重复投递。
    expect(stream.accept(deltas(patch([permission, question])))).toEqual([]);
    expect(stream.accept(deltas(patch([permission])))).toMatchObject([
      { type: "elicitation_response", requestId: "ask-1" },
    ]);
  });

  it("resumes after a gap snapshot without duplicating or losing text", () => {
    const stream = createBotConversationStream("task");
    stream.accept(snapshot([header(1, "running"), text(2, "Hello")]));
    // gap 期间正文增长、工具出现、轮次结束：重订阅快照按已交付进度续发。
    const events = stream.accept(
      snapshot([header(1, "completedSuccess"), text(2, "Hello there"), tool(3, "success")]),
    );
    // 终态必须排在补发正文与工具之后，Bot 才不会先收口再丢掉尾部内容。
    expect(types(events)).toEqual([
      "agent_message_chunk",
      "tool_call",
      "tool_call_update",
      "task_complete",
    ]);
    expect(events[0]).toMatchObject({ content: " there" });
  });
});
