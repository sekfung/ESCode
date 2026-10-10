import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fixture, event, end, type Harness } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// V4 setAssistantFeedback（助手回复点赞/点踩）在 Node 与 Rust 上的 ACK、行投影与重启后恢复一致。
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");
type Runtime = "node" | "rust";

async function feedback(h: Harness, sessionId: string, target: unknown, value: unknown) {
  const page = await h.rows(sessionId);
  const ack = await h.command({
    ...h.envelope("setAssistantFeedback", sessionId, { target, feedback: value }),
    baseRevision: page.atRevision,
    baseLogEpoch: page.atLogEpoch,
  });
  const rows = (await h.rows(sessionId)).rows as any[];
  const row = rows.find((r) => r.kind === "assistantText");
  return {
    status: ack.status,
    reasonCode: ack.reasonCode ?? null,
    feedback: row?.feedback ?? null,
  };
}

async function observe(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `escode-feedback-${kind}-`));
  const respond = (_req: any, res: any) => {
    res.writeHead(200, { "content-type": "text/event-stream" });
    event(res, { content: "answer" });
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
    await h.command(h.envelope("sendText", id, { text: "hello", mode: "yolo" }));
    await h.completed(id);
    const rows = (await h.rows(id)).rows as any[];
    const assistant = rows.find((r) => r.kind === "assistantText");
    const user = rows.find((r) => r.kind === "userInput");
    const target = { rowId: assistant.rowId, entityId: assistant.entityId };
    const out = {
      like: await feedback(h, id, target, "like"),
      likeAgain: await feedback(h, id, target, "like"),
      dislike: await feedback(h, id, target, "dislike"),
      clear: await feedback(h, id, target, null),
      userRow: await feedback(h, id, { rowId: user.rowId, entityId: user.entityId }, "like"),
      missing: await feedback(h, id, { rowId: 99_999, entityId: "missing" }, "like"),
      final: await feedback(h, id, target, "like"),
      restored: null as unknown,
    };
    assert.deepEqual(h.schemaErrors, []);
    await h.close();
    // 重启冷恢复：反馈随会话持久化。
    const restarted = f.start();
    await restarted.subscribe(`conversation/${id}`);
    const restoredRows = (await restarted.rows(id)).rows as any[];
    out.restored = restoredRows.find((r) => r.kind === "assistantText")?.feedback ?? null;
    await restarted.close();
    return out;
  } finally {
    await f.close();
  }
}

test("assistant feedback behaves the same on Node and Rust", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(rust, node);
  assert.equal(node.like.feedback, "like");
  assert.equal(node.clear.feedback, null);
  assert.equal(node.restored, "like");
});
