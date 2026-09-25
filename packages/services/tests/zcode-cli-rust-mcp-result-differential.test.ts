import assert from "node:assert/strict";
import test from "node:test";
import { join, resolve } from "node:path";
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fixture, event, end } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";
import { anthropic } from "./zcode-cli-rust-protocol-fixture.js";

// docs/specs/rust-mcp-parity.md 第 1、2 期：同一 MCP server 的各类结果（多段文本、structuredContent、
// isError、图片/音频/resource 混合）在 Node 与 Rust 上给模型的工具结果一致。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const properties = {
  contextWindow: 200000,
  inputFormat: {
    supportsText: true,
    supportsImage: true,
    supportsPdf: false,
    supportsVideo: false,
    supportsAudio: false,
  },
  outputFormat: { supportsText: true },
};
// 1×1 RGBA PNG（仅作为 MCP 图片 payload，不需要解码）。
const pngBase64 =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
const shapes: Record<string, unknown> = {
  texts: {
    content: [
      { type: "text", text: "a" },
      { type: "text", text: "" },
      { type: "text", text: "b" },
    ],
  },
  structured: { content: [{ type: "text", text: "x" }], structuredContent: { k: 1 } },
  error: { content: [{ type: "text", text: "bad" }], isError: true },
  media: {
    content: [
      { type: "text", text: "see" },
      { type: "image", mimeType: "image/png", data: pngBase64 },
      { type: "audio", mimeType: "audio/wav", data: "AAAA" },
      { type: "resource", resource: { uri: "file:///x.txt", text: "hi" } },
    ],
  },
  empty: { content: [] },
};
const kinds = Object.keys(shapes);

async function mcpServer(root: string) {
  const script = join(root, "mcp-shapes.mjs");
  await writeFile(
    script,
    `
import { createInterface } from "node:readline";
const shapes = ${JSON.stringify(shapes)};
const reply = (m,result) => process.stdout.write(JSON.stringify({jsonrpc:"2.0",id:m.id,result})+"\\n");
createInterface({input:process.stdin}).on("line",line=>{
 const m=JSON.parse(line);
 if(m.method==="initialize") reply(m,{protocolVersion:"2025-11-25",serverInfo:{name:"shapes",version:"1"},capabilities:{tools:{}}});
 else if(m.method==="server/discover")process.stdout.write(JSON.stringify({jsonrpc:"2.0",id:m.id,error:{code:-32601,message:"legacy"}})+"\\n");
 else if(m.method==="tools/list") reply(m,{tools:[{name:"shape",description:"Return a shaped result",inputSchema:{type:"object",properties:{kind:{type:"string"}},required:["kind"]}}]});
 else if(m.method==="tools/call") reply(m, shapes[m.params.arguments.kind]);
});
`,
  );
  return {
    name: "local.shapes",
    command: process.execPath,
    args: [script],
    env: [],
    protocolVersion: "auto" as const,
    timeoutMs: 5000,
  };
}

async function observe(kind: "node" | "rust", apiType: string) {
  const root = await mkdtemp(join(tmpdir(), `zcode-mcp-result-${kind}-`));
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
      step < kinds.length
        ? { name: "mcp__local_shapes__shape", input: { kind: kinds[step] } }
        : false;
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
    await h.command(h.envelope("sendText", id, { text: "try every shape" }));
    await h.completed(id);
    // 每个 kind 的工具结果：取调用之后那次请求中的最后一段工具结果（及其后的媒体消息）。
    const results = kinds.map((_, index) => {
      const body = requests[index + 1]?.messages ?? [];
      const at = body.findLastIndex(
        (m: any) =>
          m.role === "tool" ||
          (Array.isArray(m.content) && m.content.some((p: any) => p.type === "tool_result")),
      );
      return body.slice(at);
    });
    const observation = { results, schemaErrors: h.schemaErrors };
    await h.close();
    return observation;
  } finally {
    await f.close();
  }
}

for (const apiType of ["openai-chat-completions", "anthropic-messages"]) {
  test(`Node and Rust format MCP tool results the same way (${apiType})`, async () => {
    const node = await observe("node", apiType);
    const rust = await observe("rust", apiType);
    for (const [index, name] of kinds.entries())
      assert.deepEqual(rust.results[index], node.results[index], name);
    assert.deepEqual(node.schemaErrors, []);
    assert.deepEqual(rust.schemaErrors, []);
  });
}
