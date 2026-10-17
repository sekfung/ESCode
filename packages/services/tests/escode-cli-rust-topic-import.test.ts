import assert from "node:assert/strict";
import test from "node:test";
import { join } from "node:path";
import { escodeSessionMessagesResultSchema } from "@escode/shared";
import { createSqliteSessionStore } from "../../../apps/escode-cli/packages/adapters/src/storage/session-store.js";
import type { SessionId, ProjectId, WorkspaceId, MessageId, PartId } from "@escode/contracts";
import { fixture } from "./escode-cli-rust-fixture.js";

// docs/specs/rust-app-p0.md：旧 bot_topic_context 记录不重新注入模型历史（TS session-history-hydrator），
// 但 session/messages 仍按原位置投影出带 source/metadata 的 model-only 消息，供 Host readBotTopicSummaries 读取。
const SECRET = "OLD_TOPIC_SECRET";

async function seedTopicSession(f: Awaited<ReturnType<typeof fixture>>) {
  const store = createSqliteSessionStore({ dbPath: join(f.root, "ts.sqlite") });
  const session = {
    id: "topic" as SessionId,
    projectID: "fixture" as ProjectId,
    workspaceID: f.cwd as WorkspaceId,
    directory: f.cwd,
    slug: "topic",
    title: "topic task",
    titleSource: "custom" as const,
    version: "fixture",
    time: { created: 100, updated: 100 },
  };
  await store.createSession(session);
  const message = async (
    id: string,
    created: number,
    text: string,
    extra: Record<string, unknown> = {},
  ) => {
    await store.saveMessage({
      id: id as MessageId,
      sessionID: session.id,
      role: "user",
      time: { created },
      agent: "main",
      ...extra,
    } as never);
    await store.savePart({
      id: `part-${id}` as PartId,
      sessionID: session.id,
      messageID: id as MessageId,
      type: "text",
      text,
    } as never);
  };
  await message("msg-visible", 100, "visible question");
  await message("msg-topic", 101, SECRET, {
    synthetic: true,
    source: "bot_topic_context",
    visibility: "model-only",
    metadata: { checkpoint: "cp-1", displayText: "topic summary" },
  });
  for (const [type, data] of [
    ["runtime/execution_state", { mode: "yolo", planEnabled: false }],
    [
      "runtime/model_selection",
      { providerId: "fixture", modelId: "core-model", options: { reasoningLevel: "none" } },
    ],
  ] as const) {
    await store.saveSessionEntry({
      id: `${type}-topic`,
      sessionID: session.id,
      type,
      time: { created: 100, updated: 100 },
      data,
    });
  }
  store.close();
}

test("Legacy bot_topic_context stays out of model history but is projected by session/messages", async () => {
  const f = await fixture({ legacy: true });
  try {
    await seedTopicSession(f);
    const h = f.start();
    await h.subscribe("conversation/topic");
    const { messages } = await h.client.request(
      "session/messages",
      { sessionId: "topic" },
      escodeSessionMessagesResultSchema,
    );
    const topic = messages.filter((m) => m.info.source === "bot_topic_context");
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
