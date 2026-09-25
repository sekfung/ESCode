import assert from "node:assert/strict";
import test from "node:test";
import { copyFile, mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fixture } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";
import { createSqliteSessionStore } from "../../../apps/zcode-cli/packages/adapters/src/storage/session-store.js";
import type { SessionId, MessageId, ProjectId, WorkspaceId, PartId } from "@zcode/contracts";

// 模型请求语义：同一份由真实 TS store 写出的历史，Node 与 Rust 继续对话时发给模型的会话部分必须一致。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const session = "history-request" as SessionId;
const workspace = "history-request-workspace";
const turns = 6;

async function seed(path: string) {
  const store = createSqliteSessionStore({ dbPath: path });
  await store.createSession({
    id: session,
    projectID: "p" as ProjectId,
    workspaceID: workspace as WorkspaceId,
    directory: workspace,
    slug: "history",
    title: "history",
    titleSource: "custom",
    version: "fixture",
  });
  const selection = { providerId: "personal:fixture", modelId: "model-a" };
  await store.saveSessionEntry({
    id: "selection",
    sessionID: session,
    type: "runtime/model_selection",
    time: { created: 1, updated: 1 },
    data: selection,
  });
  let clock = 1;
  for (let i = 0; i < turns; i++) {
    const user = `u${i}` as MessageId;
    const assistant = `a${i}` as MessageId;
    await store.saveMessage({
      id: user,
      sessionID: session,
      role: "user",
      time: { created: (clock += 1) },
      agent: "main",
      modelSelection: selection,
    });
    await store.savePart({
      id: `${user}-t` as PartId,
      sessionID: session,
      messageID: user,
      type: "text",
      text: `question ${i}`,
    });
    await store.saveMessage({
      id: assistant,
      sessionID: session,
      role: "assistant",
      parentID: user,
      time: { created: (clock += 1), completed: clock },
      agent: "main",
      mode: "yolo",
      path: { cwd: workspace, root: workspace },
      cost: 0,
      tokens: { input: 1, output: 1, reasoning: 0, cache: { read: 0, write: 0 } },
    });
    await store.savePart({
      id: `${assistant}-tool` as PartId,
      sessionID: session,
      messageID: assistant,
      type: "tool",
      callID: `call-${i}`,
      tool: "Read",
      declarationIndex: 0,
      state: {
        status: "completed",
        input: { file_path: `f${i}.txt` },
        output: `content ${i}`,
        title: "read",
        metadata: {},
        time: { start: clock, end: clock },
      },
    });
    await store.savePart({
      id: `${assistant}-t` as PartId,
      sessionID: session,
      messageID: assistant,
      type: "text",
      text: `answer ${i}`,
    });
  }
  store.close();
}

/** 只保留会话部分：去掉 system 与 system-reminder（两侧提示词/提醒的差异由其它差分覆盖）。 */
function conversation(messages: any[]) {
  return messages
    .filter((m) => m.role !== "system")
    .filter(
      (m) =>
        !(
          m.role === "user" &&
          (typeof m.content === "string" ? m.content : JSON.stringify(m.content)).startsWith(
            "<system-reminder>",
          )
        ),
    )
    .map((m) => ({
      role: m.role,
      content:
        typeof m.content === "string"
          ? m.content
          : Array.isArray(m.content)
            ? m.content.map((p: any) => p.text ?? p.type).join("")
            : (m.content ?? ""),
      tool_calls: (m.tool_calls ?? []).map((c: any) => [
        c.id,
        c.function?.name,
        JSON.parse(c.function?.arguments || "{}"),
      ]),
      tool_call_id: m.tool_call_id ?? null,
    }));
}

async function observe(kind: "node" | "rust", source: string) {
  const root = await mkdtemp(join(tmpdir(), `zcode-hreq-${kind}-`));
  await copyFile(source, join(root, "ts.sqlite"));
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
        })
      : await fixture({ root, registry: true, legacy: true });
  try {
    await configureRegistry(f);
    const h = f.start(workspace);
    await h.subscribe(`conversation/${session}`);
    await h.command(
      h.envelope("sendText", session, {
        text: "next question",
        mode: "yolo",
        modelSelection: {
          providerId: "personal:fixture",
          modelId: "model-a",
          options: { reasoningLevel: "low" },
        },
      }),
    );
    await h.completed(session);
    const request = f.requests[0]!;
    const observation = {
      conversation: conversation(request.messages),
      tools: (request.tools ?? []).map((t: any) => t.function ?? t),
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return observation;
  } finally {
    await f.close();
  }
}

test("Node and Rust send the same conversation to the model when continuing an imported history", async () => {
  const dir = await mkdtemp(join(tmpdir(), "zcode-hreq-seed-"));
  const source = join(dir, "ts.sqlite");
  await seed(source);
  const node = await observe("node", source);
  const rust = await observe("rust", source);
  if (process.env.ZCODE_HREQ_DUMP)
    console.log(
      "DUMP",
      JSON.stringify({
        node: node.conversation,
        rust: rust.conversation,
        nodeTools: node.tools,
        rustTools: rust.tools,
      }),
    );
  assert.deepEqual(node.schemaErrors, []);
  assert.deepEqual(rust.schemaErrors, []);
  // 自检：历史确实进入了请求（6 轮问答 + 本轮问题）。
  assert(node.conversation.some((m) => m.content === "question 0"));
  assert.deepEqual(rust.conversation, node.conversation);
  // 模型可见工具面（docs/specs/rust-tool-surface.md）：名称、顺序、描述逐字一致，参数 schema 语义一致。
  // 尚未实现的工具显式列出，实现后移出。
  const pending = new Set([
    "ReadSessionContext",
    "CronCreate",
    "CronDelete",
    "CronList",
    "CronUpdate",
  ]);
  const canonical = (value: any): any =>
    Array.isArray(value)
      ? value.map(canonical)
      : value && typeof value === "object"
        ? Object.fromEntries(
            Object.keys(value)
              .sort()
              .map((key) => [key, canonical(value[key])]),
          )
        : value;
  const surface = (tools: any[]) =>
    tools
      .filter((tool) => !pending.has(tool.name))
      .map((tool) => ({
        name: tool.name,
        description: tool.description,
        parameters: canonical(tool.parameters),
      }));
  assert.deepEqual(surface(rust.tools), surface(node.tools));
});
