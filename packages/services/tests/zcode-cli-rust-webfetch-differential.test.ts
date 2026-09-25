import assert from "node:assert/strict";
import test from "node:test";
import { join, resolve } from "node:path";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fixture, event, end } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-webfetch.md：同一轮 WebFetch 在 Node 与 Rust 上发给辅助模型的处理请求、
// 回给主模型的工具结果必须一致。需要访问公网，默认跳过；ZCODE_WEBFETCH_LIVE=1 时运行。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const live = process.env.ZCODE_WEBFETCH_LIVE === "1";
const URL_UNDER_TEST = process.env.ZCODE_WEBFETCH_URL ?? "https://example.com/";

async function observe(kind: "node" | "rust") {
  const root = await mkdtemp(join(tmpdir(), `zcode-webfetch-${kind}-`));
  let processing: any;
  const respond = (req: any, res: any) => {
    const text = (m: any) =>
      typeof m.content === "string" ? m.content : JSON.stringify(m.content);
    if (req.messages.some((m: any) => text(m).includes("Web page content:"))) {
      processing = req;
      // 辅助调用可能是非流式请求（Node generateText），按请求形态应答。
      if (req.stream === false || req.stream === undefined) {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(
          JSON.stringify({
            id: "p",
            object: "chat.completion",
            choices: [
              {
                index: 0,
                message: { role: "assistant", content: "  processed summary  " },
                finish_reason: "stop",
              },
            ],
            usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
          }),
        );
        return;
      }
      res.writeHead(200, { "content-type": "text/event-stream" });
      event(res, { content: "  processed summary  " });
      end(res, "stop");
      return;
    }
    res.writeHead(200, { "content-type": "text/event-stream" });
    if (req.messages.at(-1)?.role !== "tool") {
      event(res, {
        tool_calls: [
          {
            index: 0,
            id: "call-fetch",
            type: "function",
            function: {
              name: "WebFetch",
              arguments: JSON.stringify({ url: URL_UNDER_TEST, prompt: "What is this page?" }),
            },
          },
        ],
      });
      end(res, "tool_calls");
      return;
    }
    event(res, { content: "done" });
    end(res, "stop");
  };
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          respond,
        })
      : await fixture({ root, registry: true, respond });
  try {
    await configureRegistry(f);
    const h = f.start();
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "fetch it", mode: "yolo" }));
    await h.completed(id);
    const final = f.requests.at(-1)!;
    const toolResult = final.messages.find((m: any) => m.role === "tool")?.content;
    const observation = {
      processingPrompt: processing?.messages?.map((m: any) => ({
        role: m.role,
        content: m.content,
      })),
      processingTools: processing?.tools ?? [],
      processingMaxTokens: processing?.max_tokens ?? processing?.max_completion_tokens,
      processingStream: processing?.stream,
      toolResult,
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return observation;
  } finally {
    await f.close();
  }
}

test("Node and Rust process a WebFetch page the same way", { skip: !live }, async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(node.schemaErrors, []);
  assert.deepEqual(rust.schemaErrors, []);
  // 自检：两侧确实抓到页面并调用了辅助模型。
  assert.ok(node.processingPrompt?.[0]?.content.includes("Web page content:"));
  assert.equal(node.toolResult, "processed summary");
  assert.deepEqual(rust.processingPrompt, node.processingPrompt);
  assert.deepEqual(rust.processingTools, node.processingTools);
  assert.equal(rust.processingMaxTokens, node.processingMaxTokens);
  assert.equal(rust.toolResult, node.toolResult);
});
