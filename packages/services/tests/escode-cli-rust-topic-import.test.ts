import assert from "node:assert/strict";
import test from "node:test";
import { join } from "node:path";
import { escodeSessionMessagesResultSchema } from "@escode/shared";
import { fixture } from "./escode-cli-rust-fixture.js";
import { copyLegacySeed } from "./escode-cli-rust-legacy-seed.js";

// docs/specs/rust-app-p0.md：旧 bot_topic_context 记录不重新注入模型历史（TS session-history-hydrator），
// 但 session/messages 仍按原位置投影出带 source/metadata 的 model-only 消息，保留历史只读兼容。
const SECRET = "OLD_TOPIC_SECRET";

test("Legacy bot_topic_context stays out of model history but is projected by session/messages", async () => {
  const f = await fixture({ legacy: true });
  try {
    // 旧 bot_topic_context 记录由 TS store 冻结为仓内种子。
    await copyLegacySeed("topic-import.topic", join(f.root, "ts.sqlite"), {
      workspace: f.cwd,
      root: f.root,
    });
    const h = f.start();
    await h.subscribe("conversation/topic");
    const { messages } = await h.client.request(
      "session/messages",
      { sessionId: "topic" },
      escodeSessionMessagesResultSchema,
    );
    // `source` 只在 user 消息联合成员上；先按 role 收窄，不能直接访问联合外的字段。
    const topic = messages.filter(
      (m) => m.info.role === "user" && m.info.source === "bot_topic_context",
    );
    assert.equal(topic.length, 1);
    assert.equal(topic[0]!.info.role, "user");
    assert.equal(topic[0]!.info.visibility, "model-only");
    assert.deepEqual(topic[0]!.info.metadata, { checkpoint: "cp-1", displayText: "topic summary" });
    // 原位置：在可见首问之后。
    assert.equal(messages[messages.indexOf(topic[0]!) - 1]?.info.role, "user");
    const rows = await h.rows("topic");
    assert.ok(!JSON.stringify(rows).includes(SECRET));
    await h.command(h.envelope("sendText", "topic", { text: "continue" }));
    await h.completed("topic");
    assert.ok(!JSON.stringify(f.requests.at(-1)).includes(SECRET));
    assert.ok(JSON.stringify(f.requests.at(-1)).includes("visible question"));
    assert.deepEqual(h.schemaErrors, []);
    await h.close();
  } finally {
    await f.close();
  }
});
