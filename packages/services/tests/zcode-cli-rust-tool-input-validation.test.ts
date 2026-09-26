import assert from "node:assert/strict";
import test from "node:test";
import { readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { fixture, event, end } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-tool-input-validation.md：MCP 入参按 inputSchema 校验，失败时模型收到与 Node 逐字一致的
// InputValidationError，且工具不被调用。属性按字母序声明：Rust 经 rmcp 取得的 schema 键已排序（见规格）。
process.env.ZCODE_TEST_WAIT_MS ??= "20000";
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const schema = {
  type: "object",
  properties: {
    count: { type: "integer", minimum: 1 },
    mode: { type: "string", enum: ["fast", "slow"] },
    text: { type: "string" },
  },
  required: ["text"],
  additionalProperties: false,
};
const ARGUMENTS = [
  {},
  { text: "a", extra: 1, other: 2 },
  { text: 1 },
  { text: "a", count: 1.5 },
  { mode: "medium", count: 0 },
  { text: "a", count: 0 },
  { text: "valid" },
];

async function observe(kind: "node" | "rust") {
  const seen: string[] = [];
  const respond = (req: any, res: any) => {
    res.writeHead(200, { "content-type": "text/event-stream" });
    const results = req.messages.filter((m: any) => m.role === "tool");
    if (results.length) seen.push(String(results.at(-1).content));
    const next = ARGUMENTS[results.length];
    if (next) {
      event(res, {
        tool_calls: [
          {
            index: 0,
            id: `c${results.length}`,
            type: "function",
            function: { name: "mcp__strict__run", arguments: JSON.stringify(next) },
          },
        ],
      });
      end(res, "tool_calls");
    } else {
      event(res, { content: "done" });
      end(res, "stop");
    }
  };
  const f =
    kind === "node"
      ? await fixture({
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          respond,
          mode: "yolo",
        })
      : await fixture({ registry: true, respond, mode: "yolo" });
  try {
    await configureRegistry(f, false);
    const script = join(f.root, "strict-mcp.mjs");
    const calls = join(f.root, "strict-calls");
    await writeFile(calls, "");
    await writeFile(
      script,
      `import { createInterface } from "node:readline";
import { appendFileSync } from "node:fs";
const reply = (m, result) => process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }) + "\\n");
createInterface({ input: process.stdin }).on("line", (line) => {
  const m = JSON.parse(line);
  if (m.method === "initialize") reply(m, { protocolVersion: "2025-11-25", serverInfo: { name: "strict", version: "1" }, capabilities: { tools: {} } });
  else if (m.method === "tools/list") reply(m, { tools: [{ name: "run", description: "Run", inputSchema: ${JSON.stringify(schema)} }] });
  else if (m.method === "tools/call") {
    appendFileSync(${JSON.stringify(calls)}, JSON.stringify(m.params.arguments) + "\\n");
    reply(m, { content: [{ type: "text", text: "ran" }] });
  } else if (m.id !== undefined) process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: m.id, error: { code: -32601, message: "no" } }) + "\\n");
});
`,
    );
    const h = f.start();
    const mcpServers = [
      {
        name: "strict",
        command: process.execPath,
        args: [script],
        env: [],
        protocolVersion: "auto",
        timeoutMs: 5000,
      },
    ];
    const ack = await h.command(
      h.envelope("createSession", null, { workspaceId: f.cwd, mcpServers }),
    );
    const sid = (ack.result as any).sessionId;
    await h.subscribe(`conversation/${sid}`);
    await h.command(h.envelope("sendText", sid, { text: "call" }));
    await h.completed(sid);
    await h.close();
    return { seen, calls: (await readFile(calls, "utf8")).trim().split("\n").filter(Boolean) };
  } finally {
    await f.close();
  }
}

test("MCP tool input validation errors reach the model the same way as Node", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.equal(node.seen.length, ARGUMENTS.length);
  assert.match(node.seen[0]!, /^<tool_use_error>InputValidationError: /);
  // 只有合法的调用到达 MCP server。
  assert.deepEqual(node.calls, ['{"text":"valid"}']);
  for (const [index, args] of ARGUMENTS.entries()) {
    assert.equal(rust.seen[index], node.seen[index], JSON.stringify(args));
  }
  assert.deepEqual(rust.calls, node.calls);
});
