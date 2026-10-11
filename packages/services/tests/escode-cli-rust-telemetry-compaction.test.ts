import assert from "node:assert/strict";
import test from "node:test";
import { resolve } from "node:path";
import { conversationTelemetryFactSchema } from "@escode/shared/escode-protocol-v4";
import { fixture, event, end, type Harness } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// `compaction.terminal` 遥测（TS CompactCompleted）：一轮对话后手动 /compact，两侧发出同形的压缩终态事实。
process.env.ESCODE_TEST_WAIT_MS ??= "60000";
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");

async function observe(kind: "node" | "rust") {
  const respond = (req: any, res: any) => {
    res.writeHead(200, { "content-type": "text/event-stream" });
    const summary = JSON.stringify(req.messages).includes("Summarize");
    event(res, { content: summary ? "Summary: the user said hello." : "Hello there." });
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
    const h: Harness = f.start();
    const sessionId = await h.create();
    await h.subscribe(`conversation/${sessionId}`);
    let after = h.messages.length;
    await h.command(h.envelope("sendText", sessionId, { text: "hello", mode: "yolo" }));
    await h.completed(sessionId, after);
    after = h.messages.length;
    await h.command(h.envelope("sendText", sessionId, { text: "/compact" }));
    const deadline = Date.now() + 20_000;
    while (
      !h.messages.some((m: any) => m.method === "v4/telemetry/event" && m.params?.kind === "compaction.terminal") &&
      Date.now() < deadline
    ) {
      await new Promise((done) => setTimeout(done, 100));
    }
    await h.close();
    const facts = h.messages
      .filter((m: any) => m.method === "v4/telemetry/event" && m.params?.kind === "compaction.terminal")
      .map((m: any) => m.params);
    return {
      invalid: facts.filter((fact: any) => !conversationTelemetryFactSchema.safeParse(fact).success).length,
      facts: facts.map((fact: any) => ({
        status: fact.status,
        trigger: fact.trigger,
        hasOperation: typeof fact.operationId === "string",
        hasPre: typeof fact.preCompactTokenCount === "number",
        hasPost: typeof fact.postCompactTokenCount === "number",
        modelName: fact.modelName,
        modelProvider: fact.modelProvider,
      })),
    };
  } finally {
    await f.close();
  }
}

test("Node and Rust emit the same compaction telemetry", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.equal(node.facts.length, 1);
  assert.equal(rust.invalid, 0);
  assert.deepEqual(rust.facts, node.facts);
});
