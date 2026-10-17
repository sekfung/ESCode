import { afterEach, describe, expect, it, vi } from "vitest";
import type {
  ConversationSnapshot,
  ConversationTopicFrame,
} from "@escode/shared/escode-protocol-v4";
import { observeConversationFrames } from "../src/escode-agent/conversationFrameFeed.js";

// docs/specs/bots-v4-stream.md §2：持续订阅，gap 重订阅，dispose 释放。
afterEach(() => vi.useRealTimers());

describe("conversation frame feed", () => {
  it("delivers frames, resubscribes on a seq gap and releases on dispose", async () => {
    vi.useFakeTimers();
    let wire: (value: unknown) => void = () => {};
    const disposeFrames = vi.fn();
    let subscriptions = 0;
    let ordinal = 0;
    const send = (subscriptionId: string, frame: ConversationTopicFrame) =>
      wire({
        wireVersion: 1,
        kind: "complete",
        deliveryKind: ordinal ? "online" : "initial",
        logicalFrameId: `frame-${++ordinal}`,
        logicalFrameOrdinal: ordinal,
        topic: "conversation/task",
        subscriptionId,
        frame: { ...frame, subscriptionId },
      });
    const agent = {
      onDynamicConversationFrame: () => (handler: typeof wire) => {
        wire = handler;
        return { dispose: disposeFrames };
      },
      subscribeConversationV4: vi.fn(async () => {
        const id = `sub-${++subscriptions}`;
        ordinal = 0;
        send(id, makeSnapshotFrame(id));
        return { ack: { subscriptionId: id } };
      }),
      unsubscribeConversationV4: vi.fn(async () => undefined),
    };
    const received: ConversationTopicFrame[] = [];
    const subscription = observeConversationFrames(agent as never, {
      workspacePath: "/work",
      sessionId: "task",
    })((frame) => received.push(frame));
    await vi.advanceTimersByTimeAsync(0);
    expect(received.map((frame) => frame.payload.kind)).toEqual(["snapshot"]);
    const delta = (fromSeq: number): ConversationTopicFrame => ({
      ...makeSnapshotFrame("x"),
      fromSeq,
      toSeq: fromSeq + 1,
      payload: { kind: "deltas", deltas: [] },
    });
    send("sub-1", delta(11));
    expect(received).toHaveLength(2);
    // 跳号：丢弃该帧，退订旧订阅，稍后重订阅并以新 snapshot 续上。
    send("sub-1", delta(20));
    expect(received).toHaveLength(2);
    expect(agent.unsubscribeConversationV4).toHaveBeenCalledWith(
      expect.objectContaining({ subscriptionId: "sub-1" }),
    );
    await vi.advanceTimersByTimeAsync(500);
    expect(agent.subscribeConversationV4).toHaveBeenCalledTimes(2);
    expect(received.at(-1)?.payload.kind).toBe("snapshot");
    subscription.dispose();
    expect(disposeFrames).toHaveBeenCalledOnce();
    expect(agent.unsubscribeConversationV4).toHaveBeenCalledWith(
      expect.objectContaining({ subscriptionId: "sub-2" }),
    );
  });
});

function makeSnapshotFrame(title: string): ConversationTopicFrame {
  const snapshot: ConversationSnapshot = {
    protocolVersion: 1,
    sessionId: "task",
    logEpoch: "epoch-wire",
    seq: 11,
    revision: 1,
    control: {
      phase: "running",
      sessionEnded: false,
      canStop: true,
      stopState: "stoppable",
      stopTargetKind: "assistant",
      activeWorks: [{ kind: "primaryTurn", startedAt: 1_000 }],
      lastError: null,
      apiRetry: null,
    },
    availability: {
      fork: { allowed: true },
      compact: { allowed: true },
      switchModelConfig: { allowed: true },
      setFollowupMode: { allowed: true },
      queueEdit: { allowed: true },
      sendQueuedNow: { allowed: true },
      pauseGoal: { allowed: false, reasonCode: "guard.noGoal" },
      resumeGoal: { allowed: false, reasonCode: "guard.noGoal" },
    },
    inputRouting: { mode: "enqueue" },
    meta: { title, titleSource: "custom" },
    config: {
      provider: "glm",
      model: "glm-5",
      thought: "medium",
      thoughtLevels: [],
      followupMode: "queue",
      mode: "build",
    },
    modelTransition: null,
    usage: {
      contextWindow: {
        usedTokens: 0,
        maxTokens: 200_000,
        autoCompactThresholdTokens: null,
      },
      cumulative: {
        inputTokens: 0,
        outputTokens: 0,
        cacheReadTokens: 0,
        cacheWriteTokens: 0,
      },
    },
    queue: { items: [], autoDrain: true },
    pendingInteractions: [],
    pendingCommands: [],
    backgroundWorks: [],
    // 软门禁 additive 字段:归一化形态(带 default(null))
    workspaceHookAdmission: null,
    goal: null,
    plan: null,
    rows: { window: [], totalCount: 0, firstRowId: null },
  };
  return {
    topic: "conversation/task",
    subscriptionId: "mine",
    fromSeq: 0,
    toSeq: snapshot.seq,
    sentAt: 1_700_000_000_000,
    payload: { kind: "snapshot", snapshot },
  };
}
