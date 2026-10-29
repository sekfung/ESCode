import { beforeEach, describe, expect, it, vi } from "vitest";
import type { IESCodeSessionService } from "@escode/services";
import { invalidateDeferredDraftSessionForSkillChange } from "../src/lib/escodeDraftSkillInvalidation.js";
import { useESCodeSessionStore } from "../src/store/escodeSessionStore.js";

function createSessionService(
  closeSession: IESCodeSessionService["closeSession"] = vi.fn(async () => {}),
): Pick<IESCodeSessionService, "closeSession"> {
  return { closeSession };
}

beforeEach(() => {
  useESCodeSessionStore.setState((state) => ({
    ...state,
    workspaces: {},
  }));
});

describe("invalidateDeferredDraftSessionForSkillChange", () => {
  it("invalidates and clears the current workspace draft without closing the V4 prewarm", async () => {
    const closeSession = vi.fn(async () => {});
    const workspacePath = "/workspace/app";
    const workspaceIdentity = "ssh:dev";
    useESCodeSessionStore.getState().setDraftSessionId(workspacePath, "draft-1", workspaceIdentity);

    await invalidateDeferredDraftSessionForSkillChange({
      reason: "test",
      workspacePath,
      workspaceIdentity,
      escodeSessionService: createSessionService(closeSession),
    });

    // V4 pane 是预热会话 owner；设置侧只递增失效版本，不再直接关闭 legacy session。
    expect(closeSession).not.toHaveBeenCalled();
    expect(
      useESCodeSessionStore.getState().getWorkspaceState(workspacePath, workspaceIdentity)
        .draftSessionId,
    ).toBeNull();
    expect(
      useESCodeSessionStore.getState().getWorkspaceState(workspacePath, workspaceIdentity)
        .draftRuntimeInvalidationVersion,
    ).toBe(1);
  });

  it("invalidates a protocol-v4 prewarm even when there is no legacy draft session", async () => {
    const closeSession = vi.fn(async () => {});
    const workspacePath = "/workspace/app";

    await invalidateDeferredDraftSessionForSkillChange({
      reason: "test",
      workspacePath,
      escodeSessionService: createSessionService(closeSession),
    });

    expect(closeSession).not.toHaveBeenCalled();
    expect(
      useESCodeSessionStore.getState().getWorkspaceState(workspacePath)
        .draftRuntimeInvalidationVersion,
    ).toBe(1);
  });

  it("ignores the legacy session service and still clears the draft", async () => {
    const closeSession = vi.fn(async () => {});
    const workspacePath = "/workspace/app";
    useESCodeSessionStore.getState().setDraftSessionId(workspacePath, "draft-1");

    await invalidateDeferredDraftSessionForSkillChange({
      reason: "test",
      workspacePath,
      escodeSessionService: createSessionService(closeSession),
    });

    expect(closeSession).not.toHaveBeenCalled();
    expect(
      useESCodeSessionStore.getState().getWorkspaceState(workspacePath).draftSessionId,
    ).toBeNull();
    expect(
      useESCodeSessionStore.getState().getWorkspaceState(workspacePath)
        .draftRuntimeInvalidationVersion,
    ).toBe(1);
  });
});
