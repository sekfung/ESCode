import assert from "node:assert/strict";
import test from "node:test";
import { join, resolve } from "node:path";
import { mkdir, mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fixture } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";
import { sse } from "./zcode-cli-rust-protocol-fixture.js";

// docs/specs/rust-websearch.md：声明 provider-native 搜索的 Anthropic 模型下，Node 与 Rust 暴露同样的
// WebSearch 定义、发出同样的内部搜索请求，并把服务端搜索流整理成同样的工具结果。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const SEARCH_SYSTEM = "You are an assistant for performing a web search tool use.";
const properties = (native: boolean) => ({
  contextWindow: 200000,
  supportsNativeWebSearch: native,
  inputFormat: {
    supportsText: true,
    supportsImage: false,
    supportsPdf: false,
    supportsVideo: false,
    supportsAudio: false,
  },
  outputFormat: { supportsText: true },
});
const text = (value: unknown) => (typeof value === "string" ? value : JSON.stringify(value ?? ""));

function start(res: any) {
  res.writeHead(200, { "content-type": "text/event-stream" });
  sse(res, {
    type: "message_start",
    message: {
      id: "msg",
      role: "assistant",
      content: [],
      usage: { input_tokens: 5, output_tokens: 1 },
    },
  });
}
function finish(res: any, reason: string, usage: Record<string, unknown> = {}) {
  sse(res, {
    type: "message_delta",
    delta: { stop_reason: reason },
    usage: { output_tokens: 3, ...usage },
  });
  sse(res, { type: "message_stop" });
  res.end();
}
function textBlock(res: any, index: number, chunks: string[]) {
  sse(res, { type: "content_block_start", index, content_block: { type: "text", text: "" } });
  for (const chunk of chunks)
    sse(res, { type: "content_block_delta", index, delta: { type: "text_delta", text: chunk } });
  sse(res, { type: "content_block_stop", index });
}
/** 服务端搜索流：server_tool_use → web_search_tool_result → 带 citations_delta 的文本。 */
function searchStream(res: any) {
  start(res);
  sse(res, {
    type: "content_block_start",
    index: 0,
    content_block: { type: "server_tool_use", id: "srvtoolu_1", name: "web_search", input: {} },
  });
  sse(res, {
    type: "content_block_delta",
    index: 0,
    delta: { type: "input_json_delta", partial_json: '{"query":"rust async"}' },
  });
  sse(res, { type: "content_block_stop", index: 0 });
  sse(res, {
    type: "content_block_start",
    index: 1,
    content_block: {
      type: "web_search_tool_result",
      tool_use_id: "srvtoolu_1",
      content: [
        {
          type: "web_search_result",
          url: "https://tokio.rs",
          title: "Tokio",
          encrypted_content: "enc",
          page_age: null,
        },
      ],
    },
  });
  sse(res, { type: "content_block_stop", index: 1 });
  sse(res, { type: "content_block_start", index: 2, content_block: { type: "text", text: "" } });
  sse(res, {
    type: "content_block_delta",
    index: 2,
    delta: { type: "text_delta", text: "Rust async runs on [Tokio](https://tokio.rs)" },
  });
  sse(res, {
    type: "content_block_delta",
    index: 2,
    delta: {
      type: "citations_delta",
      citation: {
        type: "web_search_result_location",
        url: "https://tokio.rs",
        title: "Tokio",
        encrypted_index: "idx",
        cited_text: "Tokio",
      },
    },
  });
  sse(res, {
    type: "content_block_delta",
    index: 2,
    delta: {
      type: "text_delta",
      text: " — see the [async book](https://rust-lang.github.io/async-book/) and [Tokio](https://TOKIO.rs).",
    },
  });
  sse(res, { type: "content_block_stop", index: 2 });
  finish(res, "end_turn", { server_tool_use: { web_search_requests: 1 } });
}

async function observe(kind: "node" | "rust", native: boolean) {
  const root = await mkdtemp(join(tmpdir(), `zcode-websearch-${kind}-`));
  // 关闭插件：官方插件 seed 的 MCP 工具两侧环境不同，与 WebSearch 无关。
  await mkdir(join(root, "workspace", ".zcode"), { recursive: true });
  await writeFile(
    join(root, "workspace", ".zcode", "config.json"),
    JSON.stringify({ plugins: { enabled: false } }),
  );
  const main: any[] = [];
  const search: { body: any; beta: unknown }[] = [];
  let f!: Awaited<ReturnType<typeof fixture>>;
  const respond = (req: any, res: any, attempt: number) => {
    if (text(req.system).includes(SEARCH_SYSTEM)) {
      search.push({ body: req, beta: f.requestHeaders[attempt - 1]?.["anthropic-beta"] });
      searchStream(res);
      return;
    }
    main.push(req);
    start(res);
    if (main.length === 1 && native) {
      sse(res, {
        type: "content_block_start",
        index: 0,
        content_block: { type: "tool_use", id: "toolu_1", name: "WebSearch", input: {} },
      });
      sse(res, {
        type: "content_block_delta",
        index: 0,
        delta: { type: "input_json_delta", partial_json: '{"query":"rust async"}' },
      });
      sse(res, { type: "content_block_stop", index: 0 });
      finish(res, "tool_use");
      return;
    }
    textBlock(res, 0, ["done"]);
    finish(res, "end_turn");
  };
  f =
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
    await configureRegistry(f, false, {
      apiType: "anthropic-messages",
      properties: properties(native),
    });
    const h = f.start();
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "search the web for rust async" }));
    await h.completed(id);
    const names = (main[0]?.tools ?? []).map((t: any) => t.name);
    const definition = (main[0]?.tools ?? []).find((t: any) => t.name === "WebSearch");
    const toolResult = (main[1]?.messages ?? [])
      .flatMap((m: any) => (Array.isArray(m.content) ? m.content : []))
      .find((p: any) => p.type === "tool_result");
    const inner = search[0]?.body;
    const observation = {
      names,
      definition,
      inner: inner && {
        system: inner.system,
        messages: inner.messages,
        tools: inner.tools,
        maxTokens: inner.max_tokens,
        stream: inner.stream,
      },
      beta: search[0]?.beta,
      toolResult: toolResult && { content: toolResult.content, isError: toolResult.is_error },
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return observation;
  } finally {
    await f.close();
  }
}

test("Node and Rust expose and run provider-native WebSearch the same way", async () => {
  const node = await observe("node", true);
  const rust = await observe("rust", true);
  // 自检：Node 暴露 WebSearch、发出内部搜索请求并把服务端流整理成带链接的结果。
  assert.ok(node.definition, JSON.stringify(node.names));
  assert.ok(node.inner, "Node issues the internal search request");
  assert.match(text(node.toolResult?.content), /tokio\.rs/);
  assert.deepEqual(rust.names, node.names);
  assert.deepEqual(rust.definition, node.definition);
  assert.deepEqual(rust.inner, node.inner);
  assert.equal(rust.beta, node.beta);
  assert.deepEqual(rust.toolResult, node.toolResult);
  assert.deepEqual(node.schemaErrors, []);
  assert.deepEqual(rust.schemaErrors, []);
});

test("Node and Rust hide WebSearch when the model has no native search", async () => {
  const node = await observe("node", false);
  const rust = await observe("rust", false);
  assert.ok(!node.names.includes("WebSearch"), JSON.stringify(node.names));
  assert.deepEqual(rust.names, node.names);
});
