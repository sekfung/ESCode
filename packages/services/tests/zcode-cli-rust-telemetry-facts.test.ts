import assert from "node:assert/strict";
import test from "node:test";
import { resolve } from "node:path";
import type { ServerResponse } from "node:http";
import { conversationTelemetryFactSchema } from "@zcode/shared/zcode-protocol-v4";
import { fixture, event, type Harness } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// `v4/telemetry/event`（TS ConversationTelemetryFactNormalizer）：桌面埋点消费的实时会话事实。一个带工具调用的回合里，
// 两侧发出的事实序列（种类、阶段、计数类字段）一致，且 Rust 的每条事实都通过 shared 的 strict schema。
process.env.ZCODE_TEST_WAIT_MS ??= "60000";
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");

function finish(res: ServerResponse, reason: string) {
  res.end(
    `data: ${JSON.stringify({ choices: [{ delta: {}, finish_reason: reason }], usage: { prompt_tokens: 100, completion_tokens: 7, total_tokens: 107 } })}\n\ndata: [DONE]\n\n`,
  );
}

async function observe(kind: "node" | "rust") {
  let n = 0;
  const respond = (_req: any, res: any) => {
    n += 1;
    res.writeHead(200, { "content-type": "text/event-stream" });
    if (n === 1) {
      event(res, { content: "Let me look." });
      event(res, {
        tool_calls: [
          { index: 0, id: "r-1", type: "function", function: { name: "TodoRead", arguments: "{}" } },
        ],
      });
      finish(res, "tool_calls");
    } else {
      event(res, { content: "Done." });
      finish(res, "stop");
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
    await h.command(h.envelope("sendText", sessionId, { text: "check todos", mode: "yolo" }));
    await h.completed(sessionId);
    await new Promise((done) => setTimeout(done, 300));
    await h.close();
    const facts = h.messages
      .filter((m: any) => m.method === "v4/telemetry/event" && m.params?.sessionId === sessionId)
      .map((m: any) => m.params);
    const invalid = facts
      .map((fact: any) => conversationTelemetryFactSchema.safeParse(fact))
      .filter((r: any) => !r.success)
      .map((r: any) => r.error.issues);
    const shape = facts
      .filter((fact: any) => !(fact.kind === "model.request.status" && fact.querySource === "session_title"))
      .map((fact: any) => ({
        kind: fact.kind,
        ...(fact.phase ? { phase: fact.phase } : {}),
        ...(fact.status ? { status: fact.status } : {}),
        ...(fact.channel ? { channel: fact.channel, firstChunk: fact.firstChunk } : {}),
        ...(fact.toolName ? { toolName: fact.toolName } : {}),
        ...(fact.kind === "usage.delta"
          ? { inputTokens: fact.inputTokens, outputTokens: fact.outputTokens, totalTokens: fact.totalTokens }
          : {}),
        ...(fact.kind === "turn.terminal" ? { toolCallCount: fact.toolCallCount, resultType: fact.resultType } : {}),
        hasSource: typeof fact.sourceCommandId === "string",
      }));
    return { shape, invalid, schemaErrors: h.schemaErrors };
  } finally {
    await f.close();
  }
}

test("Node and Rust emit the same live conversation telemetry facts", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(node.invalid, []);
  assert.deepEqual(rust.invalid, []);
  assert.ok(node.shape.length > 0);
  // 首尾固定（turn.started / turn.terminal）；步内顺序有一处已知差异：Node 在该步的 model_request_completed
  // 与 usage.delta 之前就发出工具生命周期，Rust 在模型请求结算之后。消费侧按种类聚合，比对多重集合。
  assert.deepEqual(rust.shape[0], node.shape[0]);
  assert.deepEqual(rust.shape.at(-1), node.shape.at(-1));
  const sorted = (shape: unknown[]) => shape.map((s) => JSON.stringify(s)).sort();
  assert.deepEqual(sorted(rust.shape), sorted(node.shape));
});
