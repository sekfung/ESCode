import assert from "node:assert/strict";
import test from "node:test";
import { resolve } from "node:path";
import { escodeMcpListResultSchema, type ESCodeProtocolMcpServer } from "@escode/shared";
import { fixture } from "./escode-cli-rust-fixture.js";
import { httpServer, stdioServer } from "./escode-cli-rust-mcp-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";
import { canonical, nodeOracle } from "./escode-cli-rust-golden.js";

// docs/specs/rust-mcp-parity.md「协议协商默认值」：server 对 server/discover 完全不响应时，
// stdio 在探测预算（min(5s, timeoutMs/2)）后回落 legacy；HTTP 超时则是协商失败、不回落。
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");

async function observe(kind: "node" | "rust", transport: "stdio" | "http") {
  const f =
    kind === "node"
      ? await fixture({
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
        })
      : await fixture({ registry: true });
  let closeServer = async () => {};
  try {
    await configureRegistry(f, false);
    const h = f.start();
    let config: ESCodeProtocolMcpServer;
    if (transport === "stdio") {
      const server = await stdioServer(f.cwd, "hang-discover");
      // 2s 总预算 → 探测预算 1s；Node 还要在超时后拉起正式 session，留足余量。
      config = { ...server.config, timeoutMs: 2_000 };
    } else {
      const server = await httpServer("http", { hangDiscover: true });
      closeServer = server.close;
      // 800ms 总预算 → 400ms 探测预算；两侧都应在外层超时前结束协商。
      config = { ...server.config, timeoutMs: 800 };
    }
    const listed = await h.client.request(
      "mcp/list",
      {
        workspace: { workspacePath: f.cwd, workspaceKey: f.cwd },
        mcpServers: [config],
        mode: "connect",
      },
      escodeMcpListResultSchema,
    );
    await h.close();
    const status = listed.statuses[transport === "stdio" ? "local.echo" : "remote"];
    return {
      outcome: {
        status: status?.status,
        protocolEra: status?.protocolEra,
        toolCount: status?.toolCount,
      },
      failureKind: status?.failureKind,
    };
  } finally {
    try {
      await f.close();
    } finally {
      await closeServer();
    }
  }
}

for (const transport of ["stdio", "http"] as const)
  test(`MCP ${transport} auto probe timeout matches Node`, async () => {
    const node = await nodeOracle(`probe-timeout-${transport}`, () => observe("node", transport));
    const rust = canonical(await observe("rust", transport));
    assert.deepEqual(rust.outcome, node.outcome);
    if (transport === "http") {
      // 已知分类差异：Node 对 SDK 的 RequestTimeout 用默认 network_unreachable；
      // Rust 当前把连接阶段未细分的错误统一记为 protocol_negotiation_failed。
      assert.equal(node.failureKind, "network_unreachable");
      assert.equal(rust.failureKind, "protocol_negotiation_failed");
    } else {
      assert.equal(node.failureKind, undefined);
      assert.equal(rust.failureKind, undefined);
    }
  });
