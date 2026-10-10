import assert from "node:assert/strict";
import test from "node:test";
import { resolve } from "node:path";
import { conversationTelemetryFactSchema } from "@escode/shared/escode-protocol-v4";
import { fixture, event, end, type Harness } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// `v4/telemetry/event` 的权限与子代理生命周期：build 模式下 Write 需要确认（requested → resolved），随后一个前台
// 子代理（spawned → stopped）。比对父会话这两类事实与 Rust 事实的 schema 合法性。
process.env.ESCODE_TEST_WAIT_MS ??= "60000";
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");

const call = (id: string, name: string, args: unknown) => ({
  tool_calls: [{ index: 0, id, type: "function", function: { name, arguments: JSON.stringify(args) } }],
});

async function observe(kind: "node" | "rust") {
  const main: any[] = [];
  const respond = (req: any, res: any) => {
    res.writeHead(200, { "content-type": "text/event-stream" });
    if (JSON.stringify(req.messages).includes("child task please")) {
      event(res, { content: "child done" });
      end(res, "stop");
      return;
    }
    main.push(req);
    const n = main.length;
    if (n === 1) event(res, call("w-1", "Write", { file_path: "note.txt", content: "hi" }));
    else if (n === 2) event(res, call("a-1", "Agent", { description: "helper", prompt: "child task please" }));
    else event(res, { content: "done" });
    end(res, n <= 2 ? "tool_calls" : "stop");
  };
  const f =
    kind === "node"
      ? await fixture({
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          respond,
          mode: "build",
        })
      : await fixture({ registry: true, respond, mode: "build" });
  try {
    await configureRegistry(f, false);
    const h: Harness = f.start();
    const sessionId = await h.create();
    await h.subscribe(`conversation/${sessionId}`);
    let seen = 0;
    const answered = new Set<string>();
    const pump = setInterval(() => {
      for (; seen < h.messages.length; seen++) {
        for (const delta of h.messages[seen]?.params?.frame?.payload?.deltas ?? []) {
          for (const p of delta.patch?.pendingInteractions ?? []) {
            if (p.kind !== "permission" || answered.has(p.interactionId)) continue;
            answered.add(p.interactionId);
            const once = p.payload.options.find((o: any) => /allow.?once/i.test(o.kind));
            void h.command(
              h.envelope("resolveInteraction", sessionId, {
                interactionId: p.interactionId,
                answer: { optionId: once.optionId },
              }),
            );
          }
        }
      }
    }, 20);
    await h.command(h.envelope("sendText", sessionId, { text: "write and delegate" }));
    const deadline = Date.now() + 30_000;
    while (main.length < 3 && Date.now() < deadline) {
      await new Promise((done) => setTimeout(done, 100));
    }
    await h.completed(sessionId);
    clearInterval(pump);
    await new Promise((done) => setTimeout(done, 300));
    await h.close();
    const facts = h.messages
      .filter((m: any) => m.method === "v4/telemetry/event" && m.params?.sessionId === sessionId)
      .map((m: any) => m.params);
    const invalid = facts
      .map((fact: any) => conversationTelemetryFactSchema.safeParse(fact))
      .filter((r: any) => !r.success)
      .map((r: any) => r.error.issues);
    const lifecycle = facts
      .filter((fact: any) => fact.kind === "permission.lifecycle" || fact.kind === "subagent.lifecycle")
      .map((fact: any) => ({
        kind: fact.kind,
        phase: fact.phase,
        toolCallId: fact.toolCallId,
        toolName: fact.toolName,
        decision: fact.decision,
        hasRequestId: typeof fact.requestId === "string",
        agentType: fact.agentType,
        background: fact.background,
        status: fact.status,
        parentToolCallId: fact.parentToolCallId,
        hasChild: typeof fact.childSessionId === "string",
      }));
    return { lifecycle, invalid, schemaErrors: h.schemaErrors };
  } finally {
    await f.close();
  }
}

test("Node and Rust emit the same permission and subagent lifecycle telemetry", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(node.invalid, []);
  assert.deepEqual(rust.invalid, []);
  assert.ok(node.lifecycle.length >= 3);
  assert.deepEqual(rust.lifecycle, node.lifecycle);
});
