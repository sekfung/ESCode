import assert from "node:assert/strict";
import test from "node:test";
import { writeFile } from "node:fs/promises";
import { createServer, type ServerResponse } from "node:http";
import { once } from "node:events";
import { join, resolve } from "node:path";
import { fixture, event, end } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-tool-schema-order.md：stdio 与 SSE MCP 工具的 inputSchema 按声明顺序发给模型（Node 保留原文顺序；
// Rust 之前经 rmcp 与 serde_json 排序）。fixture 用 JSON.parse 读取请求体，保留线上键顺序。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const schema = {
  type: "object",
  properties: {
    text: { type: "string", description: "Text" },
    mode: { enum: ["fast", "slow"], type: "string" },
    count: { type: "integer", minimum: 1.0, maximum: 10 },
    nested: { type: "object", properties: { zeta: { type: "string" }, alpha: { type: "number" } } },
  },
  required: ["text"],
  additionalProperties: false,
};

/** 最小 SSE MCP server：tools/list 应答逐字写出 schema 原文（含 1.0），与真实 server 的线上字节一致。 */
async function sseServer(rawSchema: string) {
  let stream: ServerResponse | undefined;
  const server = createServer(async (req, res) => {
    if (req.method === "GET") {
      stream = res;
      res.writeHead(200, { "Content-Type": "text/event-stream" });
      res.write("event: endpoint\ndata: /messages\n\n");
      return;
    }
    if (req.method === "DELETE") return void res.writeHead(204).end();
    let body = "";
    for await (const part of req) body += part;
    const m = JSON.parse(body);
    res.writeHead(202).end();
    if (m.id === undefined) return;
    const payload =
      m.method === "tools/list"
        ? `{"jsonrpc":"2.0","id":${JSON.stringify(m.id)},"result":{"tools":[{"name":"run","description":"Run","inputSchema":${rawSchema}}]}}`
        : JSON.stringify(
            m.method === "initialize"
              ? {
                  jsonrpc: "2.0",
                  id: m.id,
                  result: {
                    protocolVersion: "2025-11-25",
                    serverInfo: { name: "ordered", version: "1" },
                    capabilities: { tools: {} },
                  },
                }
              : { jsonrpc: "2.0", id: m.id, error: { code: -32601, message: "no" } },
          );
    stream?.write(`event: message\ndata: ${payload}\n\n`);
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("No fixture port");
  return {
    url: `http://127.0.0.1:${address.port}/sse`,
    async close() {
      stream?.end();
      server.closeAllConnections();
      await new Promise<void>((resolve) => server.close(() => resolve()));
    },
  };
}

async function observe(kind: "node" | "rust", transport: "stdio" | "sse") {
  let parameters = "";
  const respond = (req: any, res: any) => {
    res.writeHead(200, { "content-type": "text/event-stream" });
    const tool = (req.tools ?? []).find((t: any) => t.function.name === "mcp__ordered__run");
    parameters = JSON.stringify(tool?.function.parameters);
    event(res, { content: "done" });
    end(res, "stop");
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
    const script = join(f.root, "ordered-mcp.mjs");
    // 原文逐字写出 schema（含 1.0），与真实 server 的线上字节一致。
    const rawSchema = JSON.stringify(schema).replace('"minimum":1', '"minimum":1.0');
    await writeFile(
      script,
      `import { createInterface } from "node:readline";
const reply = (m, result) => process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }) + "\\n");
createInterface({ input: process.stdin }).on("line", (line) => {
  const m = JSON.parse(line);
  if (m.method === "initialize") reply(m, { protocolVersion: "2025-11-25", serverInfo: { name: "ordered", version: "1" }, capabilities: { tools: {} } });
  else if (m.method === "tools/list") process.stdout.write('{"jsonrpc":"2.0","id":' + JSON.stringify(m.id) + ',"result":{"tools":[{"name":"run","description":"Run","inputSchema":${rawSchema.replace(/'/g, "\\'")}}]}}\\n');
  else if (m.id !== undefined) process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: m.id, error: { code: -32601, message: "no" } }) + "\\n");
});
`,
    );
    const sse = transport === "sse" ? await sseServer(rawSchema) : undefined;
    const h = f.start();
    const mcpServers = sse
      ? [{ name: "ordered", type: "sse", url: sse.url, headers: [] }]
      : [
          {
            name: "ordered",
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
    await h.command(h.envelope("sendText", sid, { text: "hi" }));
    await h.completed(sid);
    await h.close();
    await sse?.close();
    return parameters;
  } finally {
    await f.close();
  }
}

for (const transport of ["stdio", "sse"] as const) {
  test(`${transport} MCP tool schemas reach the model in declaration order as in Node`, async () => {
    const node = await observe("node", transport);
    const rust = await observe("rust", transport);
    assert.equal(node, JSON.stringify(schema));
    assert.equal(rust, node);
  });
}
