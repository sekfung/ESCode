import { describe, expect, it, vi } from "vitest";
import { createHostRemoteWorkspaceProxyState } from "../src/host/hostRemoteWorkspaceProxyState.js";

describe("createHostRemoteWorkspaceProxyState", () => {
  it("dedupes workspace subscriptions and allows the workspace to subscribe again after release", () => {
    const state = createHostRemoteWorkspaceProxyState();
    const workspaceA = {
      workspacePath: "/work/a",
      workspaceIdentity: "remote:wsl:a",
    };
    const workspaceB = {
      workspacePath: "/work/b",
      workspaceIdentity: "remote:wsl:b",
    };
    const workspaceADispose = vi.fn();
    const workspaceBDispose = vi.fn();

    expect(
      state.ensureWorkspaceSubscription(workspaceA, () => ({ dispose: workspaceADispose })),
    ).toBe(true);
    expect(state.ensureWorkspaceSubscription(workspaceA, () => ({ dispose: vi.fn() }))).toBe(false);
    state.ensureWorkspaceSubscription(workspaceB, () => ({ dispose: workspaceBDispose }));

    state.clearWorkspace(workspaceA);

    expect(workspaceADispose).toHaveBeenCalledOnce();
    expect(workspaceBDispose).not.toHaveBeenCalled();
    expect(state.ensureWorkspaceSubscription(workspaceA, () => ({ dispose: vi.fn() }))).toBe(true);
  });
});
