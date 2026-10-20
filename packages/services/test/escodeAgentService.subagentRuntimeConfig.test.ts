import { afterEach, describe, expect, it, vi } from "vitest";
import {
  subagentRuntimeConfigSchema,
  escodeProtocolMethods,
  type SubagentRuntimeConfig,
} from "@escode/shared";
import { accountSelectionView } from "./fixtures/accountSelectionView.js";

afterEach(() => {
  vi.doUnmock("../src/escode-agent/escodeAgentProcessManager.js");
  vi.resetModules();
});

describe("subagent runtime config reverse RPC", () => {
  it("routes by the bound workspace and validates both request and complete snapshot", async () => {
    const handlers: Array<(request: { id: string; method: string; params?: unknown }) => void> = [];
    const request = (id: string, params: unknown) =>
      handlers.forEach((handler) =>
        handler({ id, method: escodeProtocolMethods.subagentsReadRuntimeConfig, params }),
      );
    const respond = vi.fn(async () => {}),
      respondError = vi.fn(async () => {});
    vi.doMock("../src/escode-agent/escodeAgentProcessManager.js", () => ({
      ESCodeAgentProcessManager: class {
        onRuntimeLifecycle() {
          return { dispose() {} };
        }
        onRuntimeRestarted() {
          return { dispose() {} };
        }
        markReady() {}
        disposeAll() {}
        async getClient() {
          return {
            request: async (_method: string, params: unknown) => params,
            respond,
            respondError,
            transportKind: "stdio",
            onNotification: () => ({ dispose() {} }),
            onClose: () => ({ dispose() {} }),
            onRequest: (handler: (typeof handlers)[number]) => {
              handlers.push(handler);
              return { dispose() {} };
            },
          };
        }
      },
    }));
    const configuration = {
      kind: "ready" as const,
      profiles: [
        {
          path: "/user/agents/reviewer.md",
          source: "user" as const,
          name: "reviewer",
          description: "Review",
          systemPrompt: "Prompt",
        },
      ],
      builtInModelSelectionOverrides: {},
      pluginAgentModelSelectionOverrides: {},
    };
    const pending = Promise.withResolvers<typeof configuration>();
    const read = vi
      .fn(async (): Promise<SubagentRuntimeConfig> => configuration)
      .mockReturnValueOnce(pending.promise);
    const { createESCodeAgentService } = await import("../src/escode-agent/escodeAgentService.js");
    const service = createESCodeAgentService({
      readSubagentRuntimeConfig: read,
      modelSelectionReadinessSource: { getView: async () => accountSelectionView() },
    });
    const workspace = { workspacePath: "/workspace/a", workspaceIdentity: "test:a" };
    try {
      await service.initialize(workspace);
      request("valid", { sessionId: "session-a" });
      await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(1));
      expect(respond).not.toHaveBeenCalled();
      pending.resolve(configuration);
      await vi.waitFor(() => expect(respond).toHaveBeenCalledWith("valid", configuration));
      expect(read).toHaveBeenCalledWith(expect.objectContaining(workspace));
      request("invalid", { sessionId: "session-a", workspacePath: "/workspace/b" });
      await vi.waitFor(() =>
        expect(respondError).toHaveBeenCalledWith(
          "invalid",
          expect.objectContaining({ code: -32602 }),
        ),
      );
      expect(read).toHaveBeenCalledTimes(1);
      read.mockResolvedValueOnce({ kind: "built-in-fallback" });
      request("fallback", { sessionId: "session-a" });
      await vi.waitFor(() =>
        expect(respond).toHaveBeenCalledWith("fallback", { kind: "built-in-fallback" }),
      );
      read.mockImplementationOnce(async () => {
        throw new Error("not prepared");
      });
      request("unavailable", { sessionId: "session-b" });
      await vi.waitFor(() =>
        expect(respondError).toHaveBeenCalledWith(
          "unavailable",
          expect.objectContaining({ code: -32603 }),
        ),
      );
      expect(
        subagentRuntimeConfigSchema.safeParse({
          documents: [{ name: "summary only" }],
          kind: "ready",
        }).success,
      ).toBe(false);
    } finally {
      service.disposeAll();
    }
  });
});
