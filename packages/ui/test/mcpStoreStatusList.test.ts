import { describe, expect, it } from "vitest";
import type { ESCodeMcpServer } from "@escode/shared";
import { mergeMcpServerStatusSnapshots } from "../src/store/mcpStoreStatusList.js";

const server: ESCodeMcpServer = {
  config: { command: "node" },
  enabled: true,
  id: "escodeagentmcp:demo",
  name: "demo",
  scope: "user",
  source: "escodeagentmcp",
  status: "connecting",
};

describe("MCP store status diagnostics", () => {
  it("preserves structured diagnostics from the runtime snapshot", () => {
    expect(
      mergeMcpServerStatusSnapshots([server], {
        demo: {
          error: "raw english error",
          failureKind: "server_unavailable",
          serverRequestId: "req-1000",
          status: "failed",
          toolCount: 0,
          transport: "http",
          updatedAt: "2026-08-22T00:00:00.000Z",
        },
      })[0],
    ).toMatchObject({
      error: "raw english error",
      failureKind: "server_unavailable",
      serverRequestId: "req-1000",
      status: "error",
    });
  });

  it("uses status_unavailable without inventing a request id for a missing row", () => {
    expect(mergeMcpServerStatusSnapshots([server], {})[0]).toMatchObject({
      failureKind: "status_unavailable",
      status: "error",
    });
    expect(mergeMcpServerStatusSnapshots([server], {})[0]).not.toHaveProperty("serverRequestId");
  });

  it("clears stale diagnostics when a later snapshot is connected", () => {
    const failedServer: ESCodeMcpServer = {
      ...server,
      error: "old error",
      failureKind: "server_unavailable",
      serverRequestId: "old-request",
      status: "error",
    };
    expect(
      mergeMcpServerStatusSnapshots([failedServer], {
        demo: {
          status: "connected",
          toolCount: 2,
          transport: "http",
          updatedAt: "2026-08-22T00:01:00.000Z",
        },
      })[0],
    ).toMatchObject({ status: "connected", toolCount: 2 });
    expect(
      mergeMcpServerStatusSnapshots([failedServer], {
        demo: {
          status: "connected",
          toolCount: 2,
          transport: "http",
          updatedAt: "2026-08-22T00:01:00.000Z",
        },
      })[0],
    ).not.toHaveProperty("serverRequestId", "old-request");
  });
});
