import assert from "node:assert/strict";
import test from "node:test";
import { resolve } from "node:path";
import { zcodeMcpListResultSchema } from "@zcode/shared";
import { fixture } from "./zcode-cli-rust-fixture.js";
import { httpServer } from "./zcode-cli-rust-mcp-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-mcp-parity.md「协议协商默认值」：未写 protocolVersion 的 HTTP 先 server/discover 再回落 initialize；
// SSE 即使写 auto 也只走 legacy initialize。Node 与 Rust 的请求方法序列一致。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");

async function methods(kind: "node" | "rust", transport: "http" | "sse", protocolVersion?: "auto") {
  const server = await httpServer(transport);
  const f =
    kind === "node"
      ? await fixture({
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
        })
      : await fixture({ registry: true });
  try {
    await configureRegistry(f, false);
    const h = f.start();
    const config = { ...server.config, ...(protocolVersion ? { protocolVersion } : {}) };
    const listed = await h.client.request(
      "mcp/list",
      {
        workspace: { workspacePath: f.cwd, workspaceKey: f.cwd },
        mcpServers: [config],
        mode: "connect",
      },
      zcodeMcpListResultSchema,
    );
    await h.close();
    return {
      status: listed.statuses.remote?.status,
      protocolEra: listed.statuses.remote?.protocolEra,
      methods: server.calls.map((c) => c.method),
    };
  } finally {
    try {
      await f.close();
    } finally {
      await server.close();
    }
  }
}

for (const [transport, protocolVersion] of [
  ["http", undefined],
  ["sse", "auto"],
] as const)
  test(`MCP ${transport} protocol negotiation with ${protocolVersion ?? "no"} protocolVersion matches Node`, async () => {
    const node = await methods("node", transport, protocolVersion);
    const rust = await methods("rust", transport, protocolVersion);
    assert.equal(node.status, "connected");
    assert.equal(node.methods.includes("server/discover"), transport === "http");
    assert.deepEqual(rust, node);
  });
