import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  GENERATION_SPEED_VIEW_TICK_MS,
  LocalGenerationSpeedTracker,
  NO_GENERATION_SPEED,
} from "@/v4/telemetry/localGenerationSpeed.js";
import type {
  AssistantTextRow,
  ConversationDelta,
  ConversationSnapshot,
  ConversationTopicFrame,
  ToolCallRow,
  TurnHeaderRow,
} from "@escode/shared/escode-protocol-v4";

const SESSION_ID = "session-speed";
const TOPIC = `conversation/${SESSION_ID}`;

/** 手动时钟：观测口径完全由注入的 now 决定，测试不依赖真实时间。 */
const clock = { now: 0 };

let seq = 0;
function deltasFrame(deltas: ConversationDelta[]): ConversationTopicFrame {
  seq += 1;
  return {
    topic: TOPIC,
    subscriptionId: "sub-1",
    fromSeq: seq - 1,
    toSeq: seq,
    sentAt: clock.now,
    payload: { kind: "deltas", deltas },
  };
}

function baseFields(rowId: number) {
  return { rowId, turnId: "turn-1", createdAt: 0, createdAtSeq: rowId };
}

function textRow(rowId: number): AssistantTextRow {
  return { ...baseFields(rowId), kind: "assistantText", text: "", state: "streaming" };
}

function toolRow(rowId: number, status: ToolCallRow["status"], extra: Partial<ToolCallRow> = {}) {
  const row: ToolCallRow = {
    ...baseFields(rowId),
    kind: "toolCall",
    toolCallId: `call-${rowId}`,
    toolName: "Read",
    status,
    inputText: "",
    ...extra,
  };
  return row;
}

/**
 * 轮末解码速度只读 CLI 投影写入的权威字段（规格 4.2）：用例通过这两个字段构造读数，
 * 不用本地估算或本机耗时去推算。
 */
function turnRow(
  rowId: number,
  state: TurnHeaderRow["state"],
  decode?: { decodeMs: number; decodeTokens: number },
) {
  const row: TurnHeaderRow = {
    ...baseFields(rowId),
    kind: "turnHeader",
    origin: "userInput",
    state,
    startedAt: 0,
  };
  if (decode) {
    row.decodeMs = decode.decodeMs;
    row.decodeTokens = decode.decodeTokens;
  }
  return row;
}

function textDelta(rowId: number, append: string, path: "text" | "inputText" = "text") {
  return { op: "row.delta", rowId, path, append } as ConversationDelta;
}

function upsert(row: AssistantTextRow | ToolCallRow | TurnHeaderRow): ConversationDelta {
  return { op: "row.upserted", row };
}

function append(row: AssistantTextRow | ToolCallRow | TurnHeaderRow): ConversationDelta {
  return { op: "row.appended", row };
}

function seededFrame(
  window: Array<AssistantTextRow | ToolCallRow | TurnHeaderRow>,
): ConversationTopicFrame {
  const snapshot = {
    rows: { window, totalCount: window.length, firstRowId: window[0]?.rowId ?? null },
    pendingInteractions: [],
  } as unknown as ConversationSnapshot;
  return {
    topic: TOPIC,
    subscriptionId: "sub-1",
    fromSeq: 0,
    toSeq: 0,
    sentAt: clock.now,
    payload: { kind: "snapshot", snapshot },
  };
}

function trackerInstance() {
  return new LocalGenerationSpeedTracker(() => clock.now);
}

/** 一个完整轮次的前缀：round 运行 + 正文行 + 首个 delta（建立生成段起点）。 */
function startTurn(tracker: LocalGenerationSpeedTracker, turnRowId: number, textRowId: number) {
  tracker.receive(deltasFrame([append(turnRow(turnRowId, "running"))]), "online");
  tracker.receive(deltasFrame([append(textRow(textRowId))]), "online");
}

beforeEach(() => {
  clock.now = 0;
  seq = 0;
});

afterEach(() => {
  vi.useRealTimers();
});

describe("LocalGenerationSpeedTracker", () => {
  it("按窗口统计实时值，起步阶段按已生成时长归一", () => {
    const tracker = trackerInstance();
    startTurn(tracker, 1, 2);

    // 6 字符 = 2 token；首个 delta 建立生成段起点，跨度仍为 0 → 不造读数。
    tracker.receive(deltasFrame([textDelta(2, "abcdef")]), "online");
    expect(tracker.read(SESSION_ID)).toEqual(NO_GENERATION_SPEED);

    // 9 字符 = 3 token；窗口 2+3=5 token，含 1s → 5 tok/s。
    clock.now = 1_000;
    tracker.receive(deltasFrame([textDelta(2, "abcdefghi")]), "online");
    expect(tracker.read(SESSION_ID)).toEqual({ mode: "live", value: 5 });

    // 满窗：所有样本都在 3 秒内，分母锁定为 3 秒；过早样本被淘汰。
    clock.now = 2_000;
    tracker.receive(deltasFrame([textDelta(2, "abcdefghi")]), "online");
    expect(tracker.read(SESSION_ID)).toEqual({ mode: "live", value: 4 });

    clock.now = 4_000;
    tracker.receive(deltasFrame([textDelta(2, "abcdefghi")]), "online");
    expect(tracker.read(SESSION_ID)).toEqual({ mode: "live", value: 2 });
  });

  it("窗口排空后回到占位符状态", () => {
    vi.useFakeTimers();
    const tracker = trackerInstance();
    startTurn(tracker, 1, 2);
    tracker.receive(deltasFrame([textDelta(2, "abcdefghi")]), "online");
    clock.now = 1_000;
    tracker.receive(deltasFrame([textDelta(2, "abcdefghi")]), "online");
    expect(tracker.read(SESSION_ID).mode).toBe("live");

    clock.now = 5_000;
    vi.advanceTimersByTime(GENERATION_SPEED_VIEW_TICK_MS);
    expect(tracker.read(SESSION_ID)).toEqual(NO_GENERATION_SPEED);
  });

  it("工具参数也是模型生成的 token，计入实时窗口", () => {
    const tracker = trackerInstance();
    startTurn(tracker, 1, 2);
    tracker.receive(deltasFrame([textDelta(2, "abcdef")]), "online");
    expect(tracker.read(SESSION_ID)).toEqual(NO_GENERATION_SPEED);

    // toolCall 行的 inputText 与正文同属一个生成段：2+3=5 token ÷ 1s。
    tracker.receive(deltasFrame([append(toolRow(3, "inputStreaming"))]), "online");
    clock.now = 1_000;
    tracker.receive(deltasFrame([textDelta(3, "abcdefghi", "inputText")]), "online");
    expect(tracker.read(SESSION_ID)).toEqual({ mode: "live", value: 5 });
  });

  it("轮末解码速度读权威字段，不用本机耗时推算", () => {
    const tracker = trackerInstance();
    startTurn(tracker, 1, 2);
    tracker.receive(deltasFrame([textDelta(2, "abc")]), "online");

    // 本机只过了 1 秒，但权威区间是 4 秒 / 100 token → 25 tok/s。
    clock.now = 1_000;
    tracker.receive(
      deltasFrame([upsert(turnRow(1, "completedSuccess", { decodeMs: 4_000, decodeTokens: 100 }))]),
      "online",
    );
    expect(tracker.read(SESSION_ID)).toEqual({ mode: "average", value: 25 });
  });

  it("缺权威字段、分母为零或分子非正时不留读数", () => {
    const cases: Array<Parameters<typeof turnRow>[2]> = [
      undefined,
      { decodeMs: 0, decodeTokens: 10 },
      { decodeMs: 4_000, decodeTokens: 0 },
    ];
    for (const decode of cases) {
      const tracker = trackerInstance();
      startTurn(tracker, 1, 2);
      tracker.receive(deltasFrame([textDelta(2, "abc")]), "online");
      clock.now = 1_000;
      tracker.receive(deltasFrame([upsert(turnRow(1, "completedSuccess", decode))]), "online");
      expect(tracker.read(SESSION_ID)).toEqual(NO_GENERATION_SPEED);
    }
  });

  it("投递缺口只清实时窗口，不影响轮末读数", () => {
    const tracker = trackerInstance();
    startTurn(tracker, 1, 2);
    tracker.receive(deltasFrame([textDelta(2, "abc")]), "online");
    clock.now = 1_000;
    tracker.receive(deltasFrame([textDelta(2, "abc")]), "online");

    // 远程 workspace重连：恢复投递重放的是错过的 deltas，不是"刚刚生成"的内容。
    clock.now = 4_000;
    tracker.receive(deltasFrame([textDelta(2, "abcdefghi")]), "recovery");
    expect(tracker.read(SESSION_ID)).toEqual(NO_GENERATION_SPEED);

    clock.now = 5_000;
    // 恢复后的首个 delta 只重建生成段起点（跨度仍为 0），随后才重新有读数。
    tracker.receive(deltasFrame([textDelta(2, "abc")]), "online");
    expect(tracker.read(SESSION_ID)).toEqual(NO_GENERATION_SPEED);

    clock.now = 6_000;
    tracker.receive(deltasFrame([textDelta(2, "abc")]), "online");
    expect(tracker.read(SESSION_ID).mode).toBe("live");

    // 漏帧只影响实时值：轮末读数由 CLI 投影累计，与客户端是否漏帧无关（规格 5.2）。
    clock.now = 7_000;
    tracker.receive(
      deltasFrame([upsert(turnRow(1, "completedSuccess", { decodeMs: 2_000, decodeTokens: 50 }))]),
      "online",
    );
    expect(tracker.read(SESSION_ID)).toEqual({ mode: "average", value: 25 });
  });

  it("桌面投影 resync（online snapshot）同样只清实时窗口", () => {
    const tracker = trackerInstance();
    startTurn(tracker, 1, 2);
    tracker.receive(deltasFrame([textDelta(2, "abc")]), "online");

    clock.now = 2_000;
    tracker.receive(seededFrame([textRow(2)]), "online");
    expect(tracker.read(SESSION_ID)).toEqual(NO_GENERATION_SPEED);

    clock.now = 3_000;
    tracker.receive(deltasFrame([textDelta(2, "abc")]), "online");
    clock.now = 4_000;
    tracker.receive(
      deltasFrame([upsert(turnRow(1, "completedSuccess", { decodeMs: 1_000, decodeTokens: 20 }))]),
      "online",
    );
    expect(tracker.read(SESSION_ID)).toEqual({ mode: "average", value: 20 });
  });

  it("新轮开始清空上一轮保留值", () => {
    const tracker = trackerInstance();
    startTurn(tracker, 1, 2);
    tracker.receive(deltasFrame([textDelta(2, "abc")]), "online");
    clock.now = 1_000;
    tracker.receive(
      deltasFrame([upsert(turnRow(1, "completedSuccess", { decodeMs: 1_000, decodeTokens: 30 }))]),
      "online",
    );
    expect(tracker.read(SESSION_ID)).toEqual({ mode: "average", value: 30 });

    clock.now = 2_000;
    startTurn(tracker, 9, 10);
    expect(tracker.read(SESSION_ID)).toEqual(NO_GENERATION_SPEED);

    tracker.receive(deltasFrame([textDelta(10, "abcdefghi")]), "online");
    clock.now = 3_000;
    tracker.receive(deltasFrame([textDelta(10, "abcdefghi")]), "online");
    expect(tracker.read(SESSION_ID).mode).toBe("live");
  });

  it("订阅时这一轮已在生成：实时值从订阅点重新累积，轮末仍读权威字段", () => {
    const tracker = trackerInstance();
    tracker.receive(seededFrame([turnRow(1, "running"), textRow(2)]), "initial");

    clock.now = 1_000;
    tracker.receive(deltasFrame([textDelta(2, "abcdefghi")]), "online");
    expect(tracker.read(SESSION_ID)).toEqual(NO_GENERATION_SPEED);

    clock.now = 2_000;
    tracker.receive(deltasFrame([textDelta(2, "abcdefghi")]), "online");
    expect(tracker.read(SESSION_ID).mode).toBe("live");

    clock.now = 3_000;
    tracker.receive(
      deltasFrame([upsert(turnRow(1, "completedSuccess", { decodeMs: 3_000, decodeTokens: 60 }))]),
      "online",
    );
    expect(tracker.read(SESSION_ID)).toEqual({ mode: "average", value: 20 });
  });

  it("订阅时无运行轮：上一轮已结算的读数不回填", () => {
    const tracker = trackerInstance();
    tracker.receive(
      seededFrame([turnRow(1, "completedSuccess", { decodeMs: 2_000, decodeTokens: 40 })]),
      "initial",
    );
    expect(tracker.read(SESSION_ID)).toEqual(NO_GENERATION_SPEED);

    clock.now = 1_000;
    startTurn(tracker, 5, 6);
    tracker.receive(deltasFrame([textDelta(6, "abc")]), "online");
    clock.now = 2_000;
    tracker.receive(deltasFrame([textDelta(6, "abc")]), "online");
    expect(tracker.read(SESSION_ID).mode).toBe("live");

    // 2 秒区间、6 token → 3 tok/s。
    clock.now = 3_000;
    tracker.receive(
      deltasFrame([upsert(turnRow(5, "completedSuccess", { decodeMs: 2_000, decodeTokens: 6 }))]),
      "online",
    );
    expect(tracker.read(SESSION_ID)).toEqual({ mode: "average", value: 3 });
  });

  it("没有生成活动的轮次（controlOnly）不显示 0", () => {
    const tracker = trackerInstance();
    clock.now = 1_000;
    tracker.receive(deltasFrame([append(turnRow(1, "running"))]), "online");
    clock.now = 2_000;
    tracker.receive(deltasFrame([upsert(turnRow(1, "completedSuccess"))]), "online");
    expect(tracker.read(SESSION_ID)).toEqual(NO_GENERATION_SPEED);
  });

  it("订阅者只在语义变化时被通知", () => {
    const tracker = trackerInstance();
    const listener = vi.fn();
    const unsubscribe = tracker.subscribe(SESSION_ID, listener);
    startTurn(tracker, 1, 2);
    tracker.receive(deltasFrame([textDelta(2, "abc")]), "online");

    clock.now = 1_000;
    tracker.receive(deltasFrame([textDelta(2, "abc")]), "online");
    const callsAfterFirstValue = listener.mock.calls.length;
    expect(callsAfterFirstValue).toBeGreaterThan(0);

    // 相同显示精度的重复帧不得再次通知（避免 Composer 高频重渲染）。
    tracker.receive(deltasFrame([upsert(textRow(2))]), "online");
    expect(listener.mock.calls.length).toBe(callsAfterFirstValue);

    unsubscribe();
    tracker.receive(deltasFrame([textDelta(2, "abcdefghi")]), "online");
    expect(listener.mock.calls.length).toBe(callsAfterFirstValue);
  });

  it("非会话 topic 被忽略", () => {
    const tracker = trackerInstance();
    tracker.receive({ ...deltasFrame([]), topic: "workspace/whatever" }, "online");
    expect(tracker.read("whatever")).toEqual(NO_GENERATION_SPEED);
  });
});
