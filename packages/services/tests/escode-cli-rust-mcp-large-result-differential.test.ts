import assert from "node:assert/strict";
import test from "node:test";
import { join, resolve } from "node:path";
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fixture, event, end } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";
import { anthropic } from "./escode-cli-rust-protocol-fixture.js";
import { canonical, nodeOracle } from "./escode-cli-rust-golden.js";

// docs/specs/rust-mcp-parity.md「大结果边界」：普通 MCP 文本结果由 Node 的通用
// resultBudget 在 50_000 字节处按 head 截断；Rust 旧实现到 64 KiB 才截断且文案不同。
// 观察按平台无关的文本长度/首尾冻结，避免把 50 KB 正文写进 golden。
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");
const properties = {
  contextWindow: 200000,
  inputFormat: {
    supportsText: true,
    supportsImage: false,
    supportsPdf: false,
    supportsVideo: false,
    supportsAudio: false,
  },
  outputFormat: { supportsText: true },
};
const shapes: Record<string, unknown> = {
  exactLimit: { content: [{ type: "text", text: "a".repeat(50_000) }] },
  overAscii: { content: [{ type: "text", text: "x".repeat(60_000) }] },
  overUnicode: { content: [{ type: "text", text: "界".repeat(20_000) }] },
};
const kinds = Object.keys(shapes);

function summarize(value: unknown): unknown {
  if (typeof value === "string" && value.length > 2_000)
    return {
      longString: {
        length: value.length,
        bytes: Buffer.byteLength(value, "utf8"),
        head: value.slice(0, 80),
        tail: value.slice(-240),
      },
    };
  if (Array.isArray(value)) return value.map(summarize);
  if (value && typeof value === "object")
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, summarize(item)]));
  return value;
}

async function mcpServer(root: string) {
  const script = join(root, "mcp-big.mjs");
  await writeFile(
    script,
    `
import { createInterface } from "node:readline";
const shapes = ${JSON.stringify(shapes)};
const reply = (m,result) => process.stdout.write(JSON.stringify({jsonrpc:"2.0",id:m.id,result})+"\\n");
createInterface({input:process.stdin}).on("line",line=>{
 const m=JSON.parse(line);
 if(m.method==="initialize") reply(m,{protocolVersion:"2025-11-25",serverInfo:{name:"big",version:"1"},capabilities:{tools:{}}});
 else if(m.method==="server/discover")process.stdout.write(JSON.stringify({jsonrpc:"2.0",id:m.id,error:{code:-32601,message:"legacy"}})+"\\n");
 else if(m.method==="tools/list") reply(m,{tools:[{name:"big",description:"Return a large result",inputSchema:{type:"object",properties:{kind:{type:"string"}},required:["kind"]}}]});
 else if(m.method==="tools/call") reply(m, shapes[m.params.arguments.kind]);
});
`,
  );
  return {
    name: "local.big",
    command: process.execPath,
    args: [script],
    env: [],
    protocolVersion: "auto" as const,
    timeoutMs: 5000,
  };
}

async function observe(kind: "node" | "rust", apiType: string) {
  const root = await mkdtemp(join(tmpdir(), `escode-mcp-large-${kind}-`));
  const requests: any[] = [];
  const respond = (req: any, res: any) => {
    if (JSON.stringify(req.messages ?? []).includes("Generate a concise title")) {
      res.writeHead(200, { "content-type": "application/json" });
      res.end(
        JSON.stringify({
          id: "t",
          object: "chat.completion",
          choices: [
            { index: 0, message: { role: "assistant", content: "Title" }, finish_reason: "stop" },
          ],
          usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
        }),
      );
      return;
    }
    requests.push(req);
    const step = requests.length - 1;
    const call =
      step < kinds.length ? { name: "mcp__local_big__big", input: { kind: kinds[step] } } : false;
    if (apiType === "anthropic-messages") return anthropic(res, call);
    res.writeHead(200, { "content-type": "text/event-stream" });
    if (call) {
      event(res, {
        tool_calls: [
          {
            index: 0,
            id: `call-${step}`,
            type: "function",
            function: { name: call.name, arguments: JSON.stringify(call.input) },
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
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          respond,
          mode: "yolo",
        })
      : await fixture({ root, registry: true, respond, mode: "yolo" });
  try {
    await configureRegistry(f, false, { apiType, properties });
    const server = await mcpServer(root);
    const h = f.start();
    const ack = await h.command(
      h.envelope("createSession", null, { workspaceId: f.cwd, mcpServers: [server] }),
    );
    const id = (ack.result as { sessionId: string }).sessionId;
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "try every large shape" }));
    await h.completed(id);
    // 每个 kind 的工具结果：取调用之后那次请求中的最后一段工具结果。
    const results = kinds.map((_, index) => {
      const body = requests[index + 1]?.messages ?? [];
      const at = body.findLastIndex(
        (m: any) =>
          m.role === "tool" ||
          (Array.isArray(m.content) && m.content.some((p: any) => p.type === "tool_result")),
      );
      return summarize(body.slice(at));
    });
    const observation = { results, schemaErrors: h.schemaErrors };
    await h.close();
    return observation;
  } finally {
    await f.close();
  }
}

for (const apiType of ["openai-chat-completions", "anthropic-messages"]) {
  test(`Node and Rust apply the same MCP result budget (${apiType})`, async () => {
    const node = await nodeOracle(`large.${apiType}`, () => observe("node", apiType));
    const rust = canonical(await observe("rust", apiType));
    for (const [index, name] of kinds.entries())
      assert.deepEqual(rust.results[index], node.results[index], name);
    assert.deepEqual(node.schemaErrors, []);
    assert.deepEqual(rust.schemaErrors, []);
  });
}
