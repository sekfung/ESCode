import { describe, expect, it } from "vitest";
import {
  createTaskNavigationHistory,
  goBack,
  goForward,
  pushAutomationsNavEntry,
  pushKnowledgeNavEntry,
  pushPluginStoreNavEntry,
  pushNavEntry,
  removeTaskFromHistory,
} from "@/lib/taskNavigationHistory.js";

describe("workspace navigation history", () => {
  it("将插件市场作为独立 workspace 目标记录", () => {
    const history = pushPluginStoreNavEntry(
      createTaskNavigationHistory(),
      "/repo",
      "ssh://dev/repo",
    );
    expect(history.entries).toEqual([
      { kind: "plugin-store", workspacePath: "/repo", workspaceIdentity: "ssh://dev/repo" },
    ]);
  });

  it("将 Automations 作为独立目标记录，并支持返回会话后再次前进", () => {
    const taskHistory = pushNavEntry(
      createTaskNavigationHistory(),
      "/workspace",
      "task-a",
      "local:/workspace",
    );
    const automationsHistory = pushAutomationsNavEntry(
      taskHistory,
      "/workspace",
      "local:/workspace",
      "automation-a",
      "idle",
    );

    const back = goBack(automationsHistory);
    expect(back?.entry).toMatchObject({
      kind: "task",
      taskId: "task-a",
      workspaceIdentity: "local:/workspace",
    });

    const forward = back ? goForward(back.history) : null;
    expect(forward?.entry).toEqual({
      kind: "automations",
      workspacePath: "/workspace",
      workspaceIdentity: "local:/workspace",
      automationId: "automation-a",
      automationTab: "idle",
    });
  });

  it("将 Automations tab intent 纳入相邻去重和前进恢复", () => {
    const scheduled = pushAutomationsNavEntry(
      createTaskNavigationHistory(),
      "/workspace",
    );
    const idle = pushAutomationsNavEntry(
      scheduled,
      "/workspace",
      undefined,
      undefined,
      "idle",
    );

    expect(idle.entries).toEqual([
      { kind: "automations", workspacePath: "/workspace" },
      { kind: "automations", workspacePath: "/workspace", automationTab: "idle" },
    ]);
    expect(
      pushAutomationsNavEntry(idle, "/workspace", undefined, undefined, "idle"),
    ).toBe(idle);
  });

  it("回放同一 Automations 目标不会重复入栈，删除 task 也不会误删 Automations", () => {
    const taskHistory = pushNavEntry(
      createTaskNavigationHistory(),
      "/workspace",
      "task-a",
    );
    const automationsHistory = pushAutomationsNavEntry(
      taskHistory,
      "/workspace",
      undefined,
      "automation-a",
    );

    expect(
      pushAutomationsNavEntry(
        automationsHistory,
        "/workspace",
        undefined,
        "automation-a",
      ),
    ).toBe(automationsHistory);

    expect(removeTaskFromHistory(automationsHistory, "task-a")).toEqual({
      entries: [
        {
          kind: "automations",
          workspacePath: "/workspace",
          automationId: "automation-a",
        },
      ],
      cursor: 0,
    });
  });

  it("将知识库主视图作为独立 workspace 目标记录，删除 task 时保留", () => {
    const taskHistory = pushNavEntry(
      createTaskNavigationHistory(),
      "/workspace",
      "task-a",
      "remote:ssh:dev:22:user:/workspace",
    );
    const knowledgeHistory = pushKnowledgeNavEntry(
      taskHistory,
      "/workspace",
      "remote:ssh:dev:22:user:/workspace",
    );
    expect(knowledgeHistory.entries).toEqual([
      {
        kind: "task",
        workspacePath: "/workspace",
        workspaceIdentity: "remote:ssh:dev:22:user:/workspace",
        taskId: "task-a",
      },
      {
        kind: "knowledge",
        workspacePath: "/workspace",
        workspaceIdentity: "remote:ssh:dev:22:user:/workspace",
      },
    ]);

    // 连续打开同一知识库目标不会重复入栈，后退再前进能恢复该页。
    expect(
      pushKnowledgeNavEntry(
        knowledgeHistory,
        "/workspace",
        "remote:ssh:dev:22:user:/workspace",
      ),
    ).toBe(knowledgeHistory);
    const back = goBack(knowledgeHistory);
    expect(back?.entry).toMatchObject({ kind: "task", taskId: "task-a" });
    expect(back ? goForward(back.history)?.entry : null).toEqual({
      kind: "knowledge",
      workspacePath: "/workspace",
      workspaceIdentity: "remote:ssh:dev:22:user:/workspace",
    });

    expect(removeTaskFromHistory(knowledgeHistory, "task-a")).toEqual({
      entries: [
        {
          kind: "knowledge",
          workspacePath: "/workspace",
          workspaceIdentity: "remote:ssh:dev:22:user:/workspace",
        },
      ],
      cursor: 0,
    });
  });
});
