import { describe, expect, it, vi } from "vitest";
import { escodeProtocolMethods } from "@escode/shared";

describe("ESCodeAgentService workspace Hook Trust", () => {
  it("没有 task/provider 时只启动 read-only Agent 并发送 workspace grant", async () => {
    const request = vi.fn(async (method: string) => {
      if (method === escodeProtocolMethods.workspaceHookTrustGrant) {
        return { accepted: true };
      }
      throw new Error(`Unexpected method ${method}`);
    });
    const getClient = vi.fn(async () => ({
      request,
      transportKind: "stdio" as const,
      onNotification: () => ({ dispose() {} }),
      onRequest: () => ({ dispose() {} }),
      onClose: () => ({ dispose() {} }),
    }));
    vi.resetModules();
    vi.doMock("../src/escode-agent/escodeAgentProcessManager.js", () => ({
      ESCodeAgentProcessManager: class {
        getClient = getClient;
        onRuntimeLifecycle() {
          return { dispose() {} };
        }
        onRuntimeRestarted() {
          return { dispose() {} };
        }
        disposeAll() {}
      },
    }));
    const { createESCodeAgentService } = await import("../src/escode-agent/escodeAgentService.js");
    const service = createESCodeAgentService();

    await expect(
      service.grantWorkspaceHookTrust({
        workspacePath: "/repo",
        workspaceIdentity: "remote:ssh:host:/repo",
        remoteSessionId: "remote-session-1",
        bundleDigest: "b".repeat(64),
        hookDeclarationDigest: "a".repeat(64),
      }),
    ).resolves.toEqual({ accepted: true });
    expect(getClient).toHaveBeenCalledTimes(1);
    expect(request).toHaveBeenCalledWith(
      escodeProtocolMethods.workspaceHookTrustGrant,
      {
        workspace: {
          workspacePath: "/repo",
          workspaceIdentity: "remote:ssh:host:/repo",
          remoteSessionId: "remote-session-1",
          workspaceKey: "remote:ssh:host:/repo",
        },
        bundleDigest: "b".repeat(64),
        hookDeclarationDigest: "a".repeat(64),
      },
      expect.anything(),
    );
    service.disposeAll();
  });
});
