// 后台 MCP App 创建入口（docs/plugin-ui-background-execution.md）。
// 断言面：live 成功 mcp_tool 结果在没有 conversation 订阅时也投递一次；失败、MCP isError
// 与历史 hydration 不产生创建副作用。移植自上游 v4-plugin-ui-background.test.ts。
import { describe, expect, it, vi } from "vitest";
import type { EventId, SessionEvent, SessionId, TraceId, TurnId } from "@escode/contracts";
import { SessionEventType } from "@escode/contracts";
import { ConversationV4Gateway } from "../src/escode-protocol-v4/index.js";

const ui = { pluginId: "editor@example", resourceUri: "ui://editor", surface: "editor" };

function buildEvents(success = true, isError = false): SessionEvent[] {
  return [
    [SessionEventType.SessionCreated, { mode: "default", contextWindow: 200_000 }],
    [SessionEventType.TurnStarted, { turnNumber: 1, input: "open", inputId: "cmd" }],
    [
      SessionEventType.ToolCallScheduled,
      {
        toolCallId: "tool",
        toolName: "mcp__editor__open",
        input: { file: "drawing.png" },
        schedule: { parallelGroups: [["tool"]], executionOrder: ["tool"] },
      },
    ],
    [
      SessionEventType.ToolCallResult,
      {
        toolCallId: "tool",
        duration: 1,
        result: {
          success,
          content: "opened",
          display: {
            kind: "mcp_tool",
            serverName: "plugin:editor:editor",
            toolName: "open",
            ui: { ...ui, ...(isError ? { isError: true as const } : {}) },
          },
        },
      },
    ],
  ].map(
    ([type, payload], index) =>
      ({
        id: `event-${index}` as EventId,
        sessionId: "session-1" as SessionId,
        turnId: "turn-1" as TurnId,
        traceId: "trace-1" as TraceId,
        type,
        payload,
        timestamp: new Date(1_700_000_000_000 + index * 1_000),
        sequenceNumber: index + 1,
      }) as SessionEvent,
  );
}

function setup(history: SessionEvent[] = []) {
  const emit = vi.fn();
  const frames = vi.fn();
  const gateway = new ConversationV4Gateway({
    sessionExists: () => true,
    executeCommand: async () => undefined,
    emitWireFrame: frames,
    emitPluginUiToolResult: emit,
    loadPersistedEvents: async () => ({ events: history, synthesized: false }),
  });
  return { gateway, emit, frames };
}

describe("后台 UI 工具结果创建入口", () => {
  it("没有 conversation 订阅也投递成功 UI 结果，重复 ingest 不重复投递", () => {
    const { gateway, emit, frames } = setup();
    try {
      const live = buildEvents();
      for (const event of live) gateway.ingest("session-1", event);
      gateway.ingest("session-1", live.at(-1)!);
      expect(frames).not.toHaveBeenCalled();
      expect(emit).toHaveBeenCalledTimes(1);
      expect(emit).toHaveBeenCalledWith({
        sessionId: "session-1",
        toolCallId: "tool",
        serverName: "plugin:editor:editor",
        ui,
        input: { file: "drawing.png" },
      });
    } finally {
      gateway.dispose();
    }
  });

  it.each([
    [false, false],
    [true, true],
  ])("失败结果不创建 success=%s isError=%s", (success, isError) => {
    const { gateway, emit } = setup();
    try {
      for (const event of buildEvents(success, isError)) gateway.ingest("session-1", event);
      expect(emit).not.toHaveBeenCalled();
    } finally {
      gateway.dispose();
    }
  });

  it("订阅历史不会重新触发创建副作用", async () => {
    const { gateway, emit } = setup(buildEvents());
    try {
      await gateway.subscribe({
        connectionId: "history",
        clientMode: "desktop-continuous",
        topic: "conversation/session-1",
      });
      expect(emit).not.toHaveBeenCalled();
    } finally {
      gateway.dispose();
    }
  });
});
