import assert from "node:assert/strict";
import test from "node:test";
import { copyFile, mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fixture } from "./zcode-cli-rust-fixture.js";
import { createSqliteSessionStore } from "../../../apps/zcode-cli/packages/adapters/src/storage/session-store.js";
import type { SessionId, MessageId, ProjectId, WorkspaceId, PartId } from "@zcode/contracts";

// docs/specs/rust-import-lifecycle.md「真实数据演练」：真实数据暴露的三种存储形态，用真实 TS store 写出，
// 再分别由 Node runtime 与 Rust 导入打开，逐行比对（不依赖用户数据）。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const session = "shape-session" as SessionId;

async function seed(path: string, workspace: string) {
  const store = createSqliteSessionStore({ dbPath: path });
  await store.createSession({
    id: session,
    projectID: "p" as ProjectId,
    workspaceID: workspace as WorkspaceId,
    directory: workspace,
    slug: "shapes",
    title: "shapes",
    titleSource: "custom",
    version: "fixture",
  });
  let clock = 10;
  const selection = { providerId: "fixture", modelId: "core-model" };
  const user = async (id: string, text: string) => {
    await store.saveMessage({
      id: id as MessageId,
      sessionID: session,
      role: "user",
      time: { created: (clock += 10) },
      agent: "main",
      modelSelection: selection,
    });
    await store.savePart({
      id: `${id}-text` as PartId,
      sessionID: session,
      messageID: id as MessageId,
      type: "text",
      text,
    });
  };
  const assistant = async (
    id: string,
    parent: string,
    parts: (msg: MessageId) => Parameters<typeof store.savePart>[0][],
  ) => {
    await store.saveMessage({
      id: id as MessageId,
      sessionID: session,
      role: "assistant",
      parentID: parent as MessageId,
      time: { created: (clock += 10), completed: clock + 1 },
      agent: "main",
      mode: "yolo",
      path: { cwd: workspace, root: workspace },
      cost: 0,
      tokens: { input: 1, output: 1, reasoning: 0, cache: { read: 0, write: 0 } },
    });
    for (const part of parts(id as MessageId)) await store.savePart(part);
  };
  const text = (msg: MessageId, id: string, value: string) => ({
    id: id as PartId,
    sessionID: session,
    messageID: msg,
    type: "text" as const,
    text: value,
  });
  const modelChange = (msg: MessageId, id: string, from: string | null, to: string) => ({
    id: id as PartId,
    sessionID: session,
    messageID: msg,
    type: "timeline" as const,
    timelineType: "model_change" as const,
    display: "separator" as const,
    status: "completed" as const,
    ...(from ? { fromModel: { providerId: "fixture", modelId: from, label: from } } : {}),
    toModel: { providerId: "fixture", modelId: to, label: to },
  });
  // 首轮之前的模型边界（Node：silentInitial，不出 marker）。
  await assistant("m0", "", (m) => [modelChange(m, "mc0", null, "core-model")]);
  await user("u1", "first question");
  // 空文本推理（部分供应商只在 metadata 保存加密推理）。
  await assistant("a1", "u1", (m) => [
    {
      id: "r1" as PartId,
      sessionID: session,
      messageID: m,
      type: "reasoning",
      text: "",
      metadata: { encrypted: "opaque" },
      time: { start: clock, end: clock + 1 },
    },
    text(m, "a1-text", "first answer"),
  ]);
  await user("u2", "discarded question");
  await assistant("a2", "u2", (m) => [text(m, "a2-text", "discarded answer")]);
  // conversation_rewind：保留 u1/a1，cut 在 a2 之后；之后追加的消息属于新分支。
  await store.setRevert({
    sessionID: session,
    revert: {
      kind: "conversation_rewind",
      scope: "conversation",
      messageID: "u2" as MessageId,
      targetMessageID: "u2" as MessageId,
      keptMessageIDs: ["m0", "u1", "a1"] as MessageId[],
      branchCutAfterMessageID: "a2" as MessageId,
      branchGeneration: 1,
    },
  });
  // 轮间换模型（Node：与上一轮模型不同 → 出 marker）。
  await assistant("m3", "", (m) => [modelChange(m, "mc3", "core-model", "other-model")]);
  await user("u3", "after rewind");
  await assistant("a3", "u3", (m) => [text(m, "a3-text", "branch answer")]);
  store.close();
}

async function observe(kind: "node" | "rust", source: string) {
  const root = await mkdtemp(join(tmpdir(), `zcode-shapes-${kind}-`));
  await copyFile(source, join(root, "ts.sqlite"));
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
        })
      : await fixture({ root, legacy: true });
  try {
    const h = f.start("shape-workspace");
    await h.subscribe(`conversation/${session}`);
    const rows = (await h.rows(session)).rows as Record<string, any>[];
    const observation = {
      kinds: rows.map((r) => r.kind),
      userTexts: rows.filter((r) => r.kind === "userInput").map((r) => r.text),
      markers: rows
        .filter((r) => r.kind === "timelineMarker")
        .map((r) => [r.marker.type, r.marker.fromModel ?? null, r.marker.toModel]),
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return observation;
  } finally {
    await f.close();
  }
}

test("Rust imports rewind branches, model changes and empty reasoning exactly as Node shows them", async () => {
  const dir = await mkdtemp(join(tmpdir(), "zcode-shapes-seed-"));
  const source = join(dir, "ts.sqlite");
  await seed(source, "shape-workspace");
  const node = await observe("node", source);
  const rust = await observe("rust", source);
  if (process.env.ZCODE_SHAPES_DUMP) console.log("DUMP", JSON.stringify({ node, rust }));
  assert.deepEqual(rust, node);
  // 自检：用例确实覆盖了三种形态（避免两侧同时为空而误判一致）。
  assert.deepEqual(node.userTexts, ["first question", "after rewind"]);
  assert(!node.kinds.includes("reasoning"), "empty reasoning must not render");
});
