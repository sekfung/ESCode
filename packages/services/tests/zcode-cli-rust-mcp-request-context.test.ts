import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { fixture, event, end } from "./zcode-cli-rust-fixture.js";
import { stdioServer } from "./zcode-cli-rust-mcp-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-browser-use.md 第 1 期：MCP tools/call 的 `_meta` 请求上下文（TS mcpRequestMeta）。
// node_repl 的浏览器 bridge 以它回到当前会话；比较键集合与取值关系（id 每次不同，不比较值）。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");

async function observe(kind: "node" | "rust") {
  const respond = (req: any, res: any) => {
    res.writeHead(200, { "content-type": "text/event-stream" });
    if (req.messages.at(-1).role !== "tool") {
      const name = req.tools.find((t: any) => t.function.name.startsWith("mcp__"))?.function.name;
      event(res, {
        tool_calls: [
          { index: 0, id: "c1", type: "function", function: { name, arguments: '{"text":"x"}' } },
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
    const server = await stdioServer(f.root);
    const h = f.start();
    const ack = await h.command(
      h.envelope("createSession", null, { workspaceId: f.cwd, mcpServers: [server.config] }),
    );
    const sid = (ack.result as any).sessionId;
    await h.subscribe(`conversation/${sid}`);
    await h.command(h.envelope("sendText", sid, { text: "call" }));
    await h.completed(sid);
    await h.close();
    const call = JSON.parse((await readFile(server.calls, "utf8")).trim().split("\n")[0]!);
    const meta = call._meta ?? {};
    const context = meta["com.zcode/request-context"] ?? {};
    const flat = Object.fromEntries(
      Object.entries(meta).filter(
        ([k]) => k !== "com.zcode/request-context" && k !== "progressToken",
      ),
    );
    return {
      keys: Object.keys(flat).sort(),
      nestedEqualsFlat:
        JSON.stringify(Object.entries(context).sort()) ===
        JSON.stringify(Object.entries(flat).sort()),
      sessionMatches: meta.session_id === sid,
      turnPresent: typeof meta.turn_id === "string" && meta.turn_id.length > 0,
      runtimeScope: meta.runtime_scope,
      workspace: meta.workspace_path === f.cwd && meta.workspace_key === f.cwd,
      clientMode: [meta.client_mode, meta.delivery_kind],
      spanLength: String(meta.span_id).length,
    };
  } finally {
    await f.close();
  }
}

test("MCP tools/call carries the same request context _meta as Node", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.equal(node.sessionMatches, true);
  assert.deepEqual(rust, node);
});
