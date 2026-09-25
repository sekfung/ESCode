import assert from "node:assert/strict";
import test from "node:test";
import { join, resolve } from "node:path";
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fixture, event, end } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";
import { zcodeSessionListResultSchema } from "@zcode/shared";
import { createSqliteSessionStore } from "../../../apps/zcode-cli/packages/adapters/src/storage/session-store.js";
import type { SessionId, MessageId, ProjectId, WorkspaceId, PartId } from "@zcode/contracts";

const SEEDED = "sess_seeded-other-workspace";

/** 另一个 workspace 的 TS 会话：Rust 不会导入它，只能经 TS 库只读回落读取。 */
async function seed(path: string) {
  const store = createSqliteSessionStore({ dbPath: path });
  await store.createSession({
    id: SEEDED as SessionId,
    projectID: "p" as ProjectId,
    workspaceID: "other-workspace" as WorkspaceId,
    directory: "/other/workspace",
    slug: "seeded",
    title: "Seeded auth work",
    titleSource: "custom",
    version: "fixture",
  });
  const add = async (id: string, role: "user" | "assistant", parts: any[], created: number) => {
    await store.saveMessage({
      id: id as MessageId,
      sessionID: SEEDED as SessionId,
      role,
      time: { created, ...(role === "assistant" ? { completed: created } : {}) },
      agent: "main",
      ...(role === "assistant"
        ? {
            mode: "yolo",
            path: { cwd: "/other/workspace", root: "/other/workspace" },
            cost: 0,
            tokens: { input: 1, output: 1, reasoning: 0, cache: { read: 0, write: 0 } },
          }
        : { modelSelection: { providerId: "personal:fixture", modelId: "model-a" } }),
    } as any);
    for (const [index, part] of parts.entries())
      await store.savePart({
        id: `${id}-${index}` as PartId,
        sessionID: SEEDED as SessionId,
        messageID: id as MessageId,
        ...part,
      });
  };
  await add(
    "m1",
    "user",
    [{ type: "text", text: "rotate the auth tokens in vault.ts" }],
    1_767_000_000_000,
  );
  await add(
    "m2",
    "assistant",
    [
      {
        type: "tool",
        callID: "c1",
        tool: "Edit",
        state: {
          status: "completed",
          input: { file_path: "vault.ts", old_string: "ttl=1h", new_string: "ttl=15m" },
          output: "edited",
          title: "Edit",
          metadata: {},
          time: { start: 1, end: 2 },
        },
      },
      { type: "text", text: "Shortened the token TTL to 15 minutes." },
    ],
    1_767_000_060_000,
  );
}

// docs/specs/rust-read-session-context.md：会话 B 引用会话 A（#sess_*）并调用 ReadSessionContext。
// 两个 runtime 的引用提醒、辅助抽取请求与工具结果，在归一化 id / 时间 / 路径后必须一致。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const text = (m: any) => (typeof m.content === "string" ? m.content : JSON.stringify(m.content));

async function observe(kind: "node" | "rust", seeded = false) {
  const root = await mkdtemp(join(tmpdir(), `zcode-sessctx-${kind}-`));
  if (seeded) await seed(join(root, "ts.sqlite"));
  let target = "";
  let lite: any;
  let referencing: any;
  const respond = (req: any, res: any) => {
    const messages = req.messages as any[];
    const title = text(messages[0] ?? {}).startsWith("Generate a concise title");
    const extraction = messages.some((m) =>
      text(m).includes("extraction model for the ReadSessionContext"),
    );
    if (title || extraction) {
      if (extraction) lite = req;
      const reply = title ? "Auth continuation" : " lite notes ";
      if (req.stream === false || req.stream === undefined) {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(
          JSON.stringify({
            id: "l",
            object: "chat.completion",
            choices: [
              { index: 0, message: { role: "assistant", content: reply }, finish_reason: "stop" },
            ],
            usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
          }),
        );
        return;
      }
      res.writeHead(200, { "content-type": "text/event-stream" });
      event(res, { content: reply });
      end(res, "stop");
      return;
    }
    res.writeHead(200, { "content-type": "text/event-stream" });
    const referencingTurn = messages.some((m) => text(m).includes("Continue from #sess_"));
    const last = messages.at(-1);
    if (last?.role !== "tool") {
      const call = referencingTurn
        ? {
            name: "ReadSessionContext",
            arguments: JSON.stringify({ sessionId: target, query: "auth tokens" }),
          }
        : { name: "Read", arguments: JSON.stringify({ file_path: "a.txt" }) };
      if (referencingTurn) referencing = req;
      event(res, { content: referencingTurn ? "" : "Let me read." });
      event(res, {
        tool_calls: [{ index: 0, id: `call-${call.name}`, type: "function", function: call }],
      });
      end(res, "tool_calls");
      return;
    }
    event(res, { content: referencingTurn ? "done" : "a.txt covers auth tokens." });
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
      : await fixture({ root, registry: true, legacy: seeded, respond });
  try {
    await writeFile(join(f.cwd, "a.txt"), "auth tokens are refreshed hourly\n");
    await configureRegistry(f);
    const h = f.start();
    if (seeded) target = SEEDED;
    else {
      target = await h.create();
      await h.subscribe(`conversation/${target}`);
      await h.command(
        h.envelope("sendText", target, {
          text: "what does a.txt say about auth tokens?",
          mode: "yolo",
        }),
      );
      await h.completed(target);
    }
    const other = await h.create();
    await h.subscribe(`conversation/${other}`);
    await h.command(
      h.envelope("sendText", other, { text: `Continue from #${target} about auth`, mode: "yolo" }),
    );
    await h.completed(other);
    const toolResult = f.requests
      .flatMap((r: any) => r.messages)
      .find((m: any) => m.role === "tool" && text(m).startsWith("ReadSessionContext"))?.content;
    // 标题生成尚未对齐（Node 走辅助模型生成，Rust 取首条输入），按各自实际标题归一化。
    const listed = (await h.client.request(
      "session/list",
      {},
      zcodeSessionListResultSchema,
    )) as any;
    const title = seeded
      ? "Seeded auth work"
      : listed.sessions.find((x: any) => x.sessionId === target)?.title || "<no-title>";
    const normalize = (value: string) =>
      value
        .replaceAll(target, "<target>")
        .replaceAll(`Target session: ${title} (`, "Target session: <title> (")
        .replaceAll(`Session: ${title} (`, "Session: <title> (")
        .replaceAll(`Title: ${title}\n`, "Title: <title>\n")
        .replaceAll(root, "<root>")
        .replaceAll(root.replaceAll("\\", "/"), "<root>")
        .replace(/\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z/g, "<time>")
        .replace(/(\] (?:user|assistant)) \S+/g, "$1 <id>");
    const observation = {
      target,
      reminder: referencing?.messages
        ?.map(text)
        .filter((t: string) => t.includes("referenced prior ZCode session"))
        .map(normalize),
      lite: lite?.messages?.map((m: any) => ({ role: m.role, content: normalize(text(m)) })),
      liteTools: lite?.tools ?? [],
      toolResult: typeof toolResult === "string" ? normalize(toolResult) : toolResult,
      schemaErrors: h.schemaErrors,
    };
    if (process.env.ZCODE_SESSCTX_DUMP)
      await writeFile(
        join(process.env.ZCODE_SESSCTX_DUMP, `${kind}.json`),
        JSON.stringify(
          { observation, referencing: referencing?.messages, lite: lite?.messages },
          null,
          1,
        ),
      );
    await h.close();
    return observation;
  } finally {
    await f.close();
  }
}

test("Node and Rust read a referenced session the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(node.schemaErrors, []);
  assert.deepEqual(rust.schemaErrors, []);
  // 会话 id 格式与 TS createSessionId 一致。
  assert.match(node.target, /^sess_/);
  assert.match(rust.target, /^sess_/);
  // 自检：两侧都注入了引用提醒、调用了辅助模型，并把 lite 结果返回给主模型。
  assert.equal(node.reminder?.length, 1);
  assert.ok(node.lite?.[1]?.content.includes("Transcript material:"));
  assert.ok(String(node.toolResult).includes("lite notes"));
  assert.deepEqual(rust.reminder, node.reminder);
  assert.deepEqual(rust.lite, node.lite);
  assert.deepEqual(rust.liteTools, node.liteTools);
  assert.equal(rust.toolResult, node.toolResult);
});

test("Node and Rust read a TS session from another workspace the same way", async () => {
  const node = await observe("node", true);
  const rust = await observe("rust", true);
  assert.deepEqual(node.schemaErrors, []);
  assert.deepEqual(rust.schemaErrors, []);
  assert.ok(node.lite?.[1]?.content.includes("rotate the auth tokens in vault.ts"));
  assert.deepEqual(rust.lite, node.lite);
  assert.equal(rust.toolResult, node.toolResult);
});
