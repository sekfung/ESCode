import { beforeEach, describe, expect, it } from "vitest";
import { useESCodeSessionStore } from "@/store/escodeSessionStore.js";

const workspace = "/workspace";

function workspaceState() {
  return useESCodeSessionStore.getState().getWorkspaceState(workspace, undefined);
}

describe("草稿组织归属", () => {
  beforeEach(() => {
    useESCodeSessionStore.setState({ workspaces: {} });
  });

  it("grouped 草稿从创建位置派生任务分组", () => {
    useESCodeSessionStore.getState().startDraft(workspace, "glm", undefined, {
      groupedDraftPlacement: { type: "group", groupId: "group-a" },
      createSource: "group",
    });
    expect(workspaceState().draftTaskGroupId).toBe("group-a");
    expect(workspaceState().draftProjectId).toBeNull();

    useESCodeSessionStore.getState().startDraft(workspace, "glm", undefined, {
      groupedDraftPlacement: { type: "top" },
    });
    expect(workspaceState().draftTaskGroupId).toBeNull();
  });

  it("显式项目/分组归属优先于 grouped 位置", () => {
    useESCodeSessionStore.getState().startDraft(workspace, "glm", undefined, {
      groupedDraftPlacement: { type: "group", groupId: "group-a" },
      projectId: "project-a",
      taskGroupId: "group-b",
    });
    expect(workspaceState().draftProjectId).toBe("project-a");
    expect(workspaceState().draftTaskGroupId).toBe("group-b");
  });

  it("关闭 grouped 草稿行会清掉分组意图", () => {
    useESCodeSessionStore.getState().startDraft(workspace, "glm", undefined, {
      groupedDraftPlacement: { type: "group", groupId: "group-a" },
    });
    useESCodeSessionStore.getState().clearGroupedDraftTask(workspace);
    expect(workspaceState().groupedDraftTask).toBeNull();
    expect(workspaceState().draftTaskGroupId).toBeNull();
  });
});
