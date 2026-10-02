import assert from "node:assert/strict";
import test from "node:test";
import { resolve } from "node:path";
import type { ServerResponse } from "node:http";
import { fixture, event, type Harness } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// `v4/conversation/usage`（TS getTaskTokenUsage → usage store queryTaskUsage）：一个带工具调用的回合（两次模型
// 请求，provider 报告含缓存命中与推理 token 的用量）之后，两侧会话用量聚合一致（增量输入基线口径）。
process.env.ZCODE_TEST_WAIT_MS ??= "60000";
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");

function finish(res: ServerResponse, reason: string, usage: Record<string, unknown>) {
  res.end(
    `data: ${JSON.stringify({ choices: [{ delta: {}, finish_reason: reason }], usage })}\n\ndata: [DONE]\n\n`,
  );
}

async function observe(kind: "node" | "rust") {
  const requests: any[] = [];
  const respond = (req: any, res: any) => {
    requests.push(req);
    res.writeHead(200, { "content-type": "text/event-stream" });
    if (requests.length === 1) {
      event(res, {
        tool_calls: [
          { index: 0, id: "g-1", type: "function", function: { name: "Glob", arguments: JSON.stringify({ pattern: "*.md" }) } },
        ],
      });
      finish(res, "tool_calls", {
        prompt_tokens: 1000,
        completion_tokens: 50,
        total_tokens: 1050,
        prompt_tokens_details: { cached_tokens: 200 },
        completion_tokens_details: { reasoning_tokens: 10 },
      });
    } else {
      event(res, { content: "done" });
      finish(res, "stop", { prompt_tokens: 1300, completion_tokens: 40, total_tokens: 1340 });
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
    const h: Harness = f.start();
    const sessionId = await h.create();
    await h.subscribe(`conversation/${sessionId}`);
    const parse = { parse: (value: unknown) => value } as any;
    const before = await h.client.request("v4/conversation/usage", { sessionId }, parse);
    await h.command(h.envelope("sendText", sessionId, { text: "list docs", mode: "yolo" }));
    await h.completed(sessionId);
    await new Promise((done) => setTimeout(done, 500));
    const after = (await h.client.request("v4/conversation/usage", { sessionId }, parse)) as any;
    // 应用级统计（v4/usage/stats）：耗时类字段两侧不同，抹成 0 / null 只比结构与计数。
    const timing = /^(generatedAt|avgTimeToFirstTokenMs|avgTurnDurationMs|longestSessionMs|avgDurationMs)$/;
    const stats = JSON.parse(
      JSON.stringify(
        await h.client.request("v4/usage/stats", { range: "7d", timeZone: "UTC" }, parse),
        (key, value) => (timing.test(key) ? (value === null ? null : 0) : value),
      ),
    );
    await h.close();
    return {
      before: { ...(before as any), sessionId: "<sid>" },
      after: { ...after, sessionId: "<sid>" },
      stats,
      toolResult: requests[1]?.messages?.find((m: any) => m.tool_call_id === "g-1")?.content,
      hasGlob: (requests[0]?.tools ?? []).some((t: any) => t.function?.name === "Glob"),
      requestCount: requests.length,
      schemaErrors: h.schemaErrors,
    };
  } finally {
    await f.close();
  }
}

test("Node and Rust aggregate conversation and app usage the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(node.schemaErrors, []);
  assert.ok(node.after.modelRequestCount >= 2);
  for (const key of Object.keys(node)) {
    assert.deepEqual((rust as any)[key], (node as any)[key], key);
  }
});
