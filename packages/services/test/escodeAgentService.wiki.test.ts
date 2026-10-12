import { describe, expect, it, vi } from "vitest";
import { V4_WIKI_METHODS } from "@escode/shared/escode-protocol-v4";
import { offPeakSelectionView } from "./fixtures/offPeakSelection.js";

// LLM Wiki 协议（apps/escode-cli-rust/docs/llm-wiki/01-spec.md §11）：只有 Rust runtime 实现；
// TS runtime / 旧 CLI 的 -32601 在服务层折算为 supported=false，其他错误照常抛出。

class MockProcessManagerBase {
  onRuntimeLifecycle() {
    return { dispose() {} };
  }
  onRuntimeRestarted() {
    return { dispose() {} };
  }
  markReady() {}
}

async function setup(handler: (method: string, params: unknown) => unknown) {
  const request = vi.fn(async (method: string, params?: unknown) => handler(method, params));
  const getClient = vi.fn(async () => ({
    request,
    transportKind: "stdio" as const,
    onNotification: () => ({ dispose() {} }),
    onRequest: () => ({ dispose() {} }),
    onClose: () => ({ dispose() {} }),
  }));
  vi.resetModules();
  vi.doMock("../src/escode-agent/escodeAgentProcessManager.js", () => ({
    ESCodeAgentProcessManager: class extends MockProcessManagerBase {
      getClient = getClient;
      disposeAll() {}
    },
  }));
  const { createESCodeAgentService } = await import("../src/escode-agent/escodeAgentService.js");
  const service = createESCodeAgentService({
    modelSelectionReadinessSource: { getView: async () => offPeakSelectionView() },
  });
  return { service, request };
}

const target = { workspacePath: "/workspace/app", workspaceIdentity: "id-app" };

describe("escodeAgentService wiki methods", () => {
  it("maps method-not-found to an unsupported status", async () => {
    const { service } = await setup(() => {
      throw Object.assign(new Error("Method not found"), { code: -32601 });
    });
    await expect(service.getWikiStatus!(target)).resolves.toEqual({
      supported: false,
      enabled: false,
    });
  });

  it("propagates other runtime errors", async () => {
    const { service } = await setup(() => {
      throw Object.assign(new Error("boom"), { code: -32603 });
    });
    await expect(service.getWikiStatus!(target)).rejects.toThrow("boom");
  });

  it("sends validated graph, entry and manage requests", async () => {
    const { service, request } = await setup((method, params) => {
      if (method === V4_WIKI_METHODS.graph) {
        return {
          enabled: true,
          project: { id: "p", name: "app" },
          nodes: [],
          edges: [],
          truncated: false,
          counts: {},
        };
      }
      if (method === V4_WIKI_METHODS.entry) return { found: false };
      if (method === V4_WIKI_METHODS.manage) {
        return {
          action: (params as { action: string }).action,
          ok: true,
          summary: "1 approve request(s)",
          items: [],
        };
      }
      throw new Error(`Unexpected method ${method}`);
    });
    await service.getWikiGraph!({ ...target, query: { statuses: ["proposal"], limit: 10 } });
    await service.getWikiEntry!({ ...target, id: "api-errors" });
    await expect(
      service.manageWiki!({ ...target, request: { action: "approve", ids: ["api-errors"] } }),
    ).resolves.toMatchObject({ ok: true });
    const calls = request.mock.calls.map(([method, params]) => [method, params]);
    expect(calls).toContainEqual([V4_WIKI_METHODS.graph, { statuses: ["proposal"], limit: 10 }]);
    expect(calls).toContainEqual([V4_WIKI_METHODS.entry, { id: "api-errors" }]);
    expect(calls).toContainEqual([
      V4_WIKI_METHODS.manage,
      { action: "approve", ids: ["api-errors"] },
    ]);
  });

  it("rejects graph params that violate the strict schema before sending", async () => {
    const { service, request } = await setup(() => ({}));
    await expect(service.getWikiGraph!({ ...target, query: { limit: 5000 } })).rejects.toThrow();
    expect(request.mock.calls.some(([method]) => method === V4_WIKI_METHODS.graph)).toBe(false);
  });
});
