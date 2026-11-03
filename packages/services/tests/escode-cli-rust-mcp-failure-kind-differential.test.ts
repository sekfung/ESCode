import assert from "node:assert/strict";
import test from "node:test";
import { createServer } from "node:http";
import { once } from "node:events";
import { resolve } from "node:path";
import { escodeMcpListResultSchema, type ESCodeProtocolMcpServer } from "@escode/shared";
import { fixture } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";
import { canonical, nodeOracle } from "./escode-cli-rust-golden.js";

// docs/specs/rust-mcp-parity.md「stdio 握手前退出」：Node 对 stdio 的默认 failureKind 是
// process_start_failed，只有版本协商类错误才改写为 protocol_negotiation_failed；Rust 也必须
// 把子进程在握手前退出归到 process_start_failed，并与 HTTP/SSE 的连接拒绝分类保持一致。
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");

async function closedPort(): Promise<number> {
  const server = createServer();
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : 0;
  await new Promise<void>((done) => server.close(() => done()));
  return port;
}

async function observe(kind: "node" | "rust", config: ESCodeProtocolMcpServer) {
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
    const status = listed.statuses[config.name];
    return { status: status?.status, failureKind: status?.failureKind };
  } finally {
    await f.close();
  }
}

async function configFor(name: string): Promise<ESCodeProtocolMcpServer> {
  const port = await closedPort();
  switch (name) {
    case "http-refused":
      return {
        name: "remote",
        type: "http",
        url: `http://127.0.0.1:${port}/mcp`,
        headers: [],
        timeoutMs: 1_200,
      };
    case "sse-refused":
      return {
        name: "remote",
        type: "sse",
        url: `http://127.0.0.1:${port}/sse`,
        headers: [],
        timeoutMs: 1_200,
      };
    case "stdio-exit":
      return {
        name: "local.echo",
        command: process.execPath,
        args: ["-e", "process.exit(1)"],
        env: [],
        timeoutMs: 1_200,
      };
    default:
      return {
        name: "local.echo",
        command: "/nonexistent/escode-mcp",
        args: [],
        env: [],
        timeoutMs: 1_200,
      };
  }
}

for (const name of ["http-refused", "sse-refused", "stdio-exit", "stdio-missing"] as const)
  test(`MCP ${name} failureKind matches Node`, async () => {
    const config = await configFor(name);
    const node = await nodeOracle(name, () => observe("node", config));
    const rust = canonical(await observe("rust", config));
    assert.deepEqual(rust, node);
  });
