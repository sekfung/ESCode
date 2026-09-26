import assert from "node:assert/strict";
import test from "node:test";
import { join, resolve } from "node:path";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { setTimeout as delay } from "node:timers/promises";
import { fixture, event, end, type Harness } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-session-title.md：会话标题 sidecar。Node 与 Rust 必须发出同样的标题请求
// （system 首段、user 内容、无工具），并在清洗后写回同样的 meta（title/titleSource）。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const text = (message: any): string =>
  typeof message?.content === "string" ? message.content : JSON.stringify(message?.content ?? "");

/** 标题请求的返回形态，覆盖 JSON、围栏、首行、工具调用与空标题。 */
type Reply = { content?: string; calls?: unknown[] };
function reply(req: any, res: any, message: Reply) {
  const calls = (message.calls ?? []) as any[];
  if (req.stream === false || req.stream === undefined) {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(
      JSON.stringify({
        id: "x",
        object: "chat.completion",
        choices: [
          {
            index: 0,
            message: {
              role: "assistant",
              content: message.content ?? "",
              ...(calls.length
                ? {
                    tool_calls: calls.map((c, index) => ({
                      index,
                      id: c.id,
                      type: "function",
                      function: { name: c.name, arguments: JSON.stringify(c.input) },
                    })),
                  }
                : {}),
            },
            finish_reason: calls.length ? "tool_calls" : "stop",
          },
        ],
        usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
      }),
    );
    return;
  }
  res.writeHead(200, { "content-type": "text/event-stream" });
  event(res, calls.length ? { tool_calls: calls } : { content: message.content ?? "" });
  end(res, calls.length ? "tool_calls" : "stop");
}

/** 会话 meta 的最后一次投影（snapshot 或 patch.meta）。 */
function metaOf(h: Harness, sessionId: string) {
  let meta: any = null;
  for (const message of h.messages) {
    if (message.params?.topic !== `conversation/${sessionId}`) continue;
    const payload = message.params.frame?.payload;
    if (payload?.snapshot?.meta) meta = payload.snapshot.meta;
    for (const delta of payload?.deltas ?? []) {
      if (delta.patch?.meta) meta = delta.patch.meta;
    }
  }
  return meta;
}

const LONG = "please refactor the login page validation";
const SHORT = "hi";
const TOOLCALL = "summarize the parser module for me";
const RENAMED = "rename the release checklist items";

async function observe(kind: "node" | "rust") {
  const root = await mkdtemp(join(tmpdir(), `zcode-title-${kind}-`));
  const titles: { input: string; system: string; user: string; tools: number }[] = [];
  // 第 4 个会话（RENAMED）的标题请求挂起，等 renameSession 之后再回答。
  let releaseRename!: () => void;
  const renameGate = new Promise<void>((resolveGate) => {
    releaseRename = resolveGate;
  });
  const respond = (req: any, res: any) => {
    const first = text(req.messages?.[0] ?? {});
    if (!first.startsWith("Generate a concise title")) {
      return reply(req, res, { content: "ok" });
    }
    const user = text(req.messages?.[1] ?? {});
    titles.push({
      input: user,
      system: first,
      user,
      tools: (req.tools ?? []).length,
    });
    if (user === RENAMED) {
      void renameGate.then(() => reply(req, res, { content: '{"title":"Renamed session"}' }));
      return;
    }
    if (user === TOOLCALL) {
      return reply(req, res, {
        calls: [{ id: "t1", name: "Read", input: { file_path: "parser.ts" } }],
      });
    }
    if (user === LONG)
      return reply(req, res, { content: '```json\n{"title":"Login validation refactor"}\n```' });
    return reply(req, res, { content: '"Plain first line" trailing' });
  };
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          respond,
          mode: "yolo",
          titleRequests: "respond",
        })
      : await fixture({ root, registry: true, respond, mode: "yolo", titleRequests: "respond" });
  try {
    await configureRegistry(f);
    const h = f.start();
    const send = async (label: string, input: string) => {
      const id = await h.create();
      await h.subscribe(`conversation/${id}`);
      await h.command(h.envelope("sendText", id, { text: input }));
      await h.completed(id);
      await delay(600);
      return { id, meta: metaOf(h, id) };
    };
    const long = await send("long", LONG);
    const short = await send("short", SHORT);
    const toolCall = await send("toolcall", TOOLCALL);
    // renameSession 与标题写回竞争：用户显式命名优先。
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: RENAMED }));
    await h.completed(id);
    for (let i = 0; i < 100 && !titles.some((t) => t.user === RENAMED); i++) await delay(50);
    await h.command(h.envelope("renameSession", id, { title: "Custom title" }));
    await delay(200);
    releaseRename();
    await delay(600);
    const renamed = { id, meta: metaOf(h, id) };
    // 第二轮输入不再触发标题请求。
    await h.command(h.envelope("sendText", long.id, { text: "second turn please" }));
    await h.completed(long.id);
    await delay(600);
    const observation = {
      titles: titles.map((t) => ({ input: t.input, tools: t.tools })),
      system: titles[0]?.system,
      longMeta: long.meta,
      shortMeta: short.meta,
      toolCallMeta: toolCall.meta,
      renamedMeta: renamed.meta,
      secondTurnMeta: metaOf(h, long.id),
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return observation;
  } finally {
    await f.close();
  }
}

test("Node and Rust generate session titles the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  // 自检：Node 为长输入发出标题请求，短输入不发；清洗后的标题与 titleSource 已投影。
  assert.deepEqual(
    node.titles.map((t) => t.input),
    [LONG, TOOLCALL, RENAMED],
    JSON.stringify(node.titles),
  );
  assert.equal(node.longMeta.title, "Login validation refactor");
  assert.equal(node.longMeta.titleSource, "generated");
  assert.equal(node.shortMeta.title, SHORT);
  assert.equal(node.renamedMeta.title, "Custom title");
  assert.equal(node.renamedMeta.titleSource, "custom");
  // 两侧请求与投影一致。
  assert.equal(rust.system, node.system);
  assert.deepEqual(rust.titles, node.titles);
  assert.deepEqual(rust.longMeta, node.longMeta);
  assert.deepEqual(rust.shortMeta, node.shortMeta);
  assert.deepEqual(rust.toolCallMeta, node.toolCallMeta);
  assert.deepEqual(rust.renamedMeta, node.renamedMeta);
  assert.deepEqual(rust.secondTurnMeta, node.secondTurnMeta);
  assert.deepEqual(rust.schemaErrors, []);
  assert.deepEqual(node.schemaErrors, []);
});

// `/goal`：TS 在 control-only turn 边界立即启动标题 sidecar；首条输入同时写会话标题与目标摘要，
// 已有标题的会话只生成目标摘要（无 10 字符门槛），空标题时写兜底摘要（归一后的目标文本）。
const GOAL_FIRST = "refactor the parser module for clarity";
const GOAL_LATER = "fix it";
const GOAL_EMPTY = "tidy up";
function goalOf(h: Harness, sessionId: string) {
  let goal: any = null;
  for (const message of h.messages) {
    if (message.params?.topic !== `conversation/${sessionId}`) continue;
    const payload = message.params.frame?.payload;
    if (payload?.snapshot && "goal" in payload.snapshot) goal = payload.snapshot.goal;
    for (const delta of payload?.deltas ?? []) {
      if (delta.patch && "goal" in delta.patch) goal = delta.patch.goal;
    }
  }
  return goal;
}

async function observeGoals(kind: "node" | "rust") {
  const root = await mkdtemp(join(tmpdir(), `zcode-goal-title-${kind}-`));
  const titles: string[] = [];
  const respond = (req: any, res: any) => {
    const first = text(req.messages?.[0] ?? {});
    if (first.startsWith("Generate a concise title")) {
      const user = text(req.messages?.[1] ?? {});
      titles.push(user);
      const reply_ =
        user === GOAL_FIRST
          ? '{"title":"Parser refactor"}'
          : user === GOAL_LATER
            ? '{"title":"Quick fix"}'
            : user === GOAL_EMPTY
              ? ""
              : '{"title":"Warmup session"}';
      return reply(req, res, { content: reply_ });
    }
    const verify = text(req.messages?.at(-1) ?? {}).includes(
      "Verify whether the active session goal",
    );
    return reply(req, res, {
      content: verify ? '{"passed":true,"reason":"done","nextAction":""}' : "ok",
    });
  };
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          respond,
          mode: "yolo",
          titleRequests: "respond",
        })
      : await fixture({ root, registry: true, respond, mode: "yolo", titleRequests: "respond" });
  try {
    await configureRegistry(f);
    const h = f.start();
    const settle = async (id: string, goalText: string) => {
      await h.command(h.envelope("sendGoalCommand", id, { text: goalText }));
      for (let i = 0; i < 200 && goalOf(h, id)?.status !== "verified"; i++) await delay(50);
      await delay(800);
    };
    const first = await h.create();
    await h.subscribe(`conversation/${first}`);
    await settle(first, GOAL_FIRST);
    const later = await h.create();
    await h.subscribe(`conversation/${later}`);
    await h.command(h.envelope("sendText", later, { text: "warm up the session first" }));
    await h.completed(later);
    await delay(800);
    await settle(later, GOAL_LATER);
    const empty = await h.create();
    await h.subscribe(`conversation/${empty}`);
    await settle(empty, GOAL_EMPTY);
    const pick = (id: string) => {
      const goal = goalOf(h, id);
      return { meta: metaOf(h, id), status: goal?.status, summaryTitle: goal?.summaryTitle };
    };
    // rust-row-projection.md：/goal 的可见 query 轮为 controlOnly（无工时），执行属于随后的 goalContinuation 轮。
    const headers = (await h.rows(first)).rows
      .filter((r: any) => r.kind === "turnHeader")
      .map((r: any) => ({
        origin: r.origin,
        executionKind: r.executionKind,
        activeMs: typeof r.activeMs,
        historyRoundCount: r.historyRoundCount,
      }));
    const all = (await h.rows(first)).rows as any[];
    const turns = [...new Set(all.map((r) => r.turnId))];
    const goalRows = all.map((r) => [
      turns.indexOf(r.turnId),
      r.kind,
      r.origin ?? r.marker?.type ?? "",
      r.executionKind ?? "",
      r.state ?? r.status ?? "",
      r.actions ?? null,
    ]);
    const observation = {
      headers,
      goalRows,
      titles: [...titles].sort(),
      first: pick(first),
      later: pick(later),
      empty: pick(empty),
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return observation;
  } finally {
    await f.close();
  }
}

test("Node and Rust title goals and their summaries the same way", async () => {
  const node = await observeGoals("node");
  const rust = await observeGoals("rust");
  // 自检：Node 为首条 /goal 同时写会话标题与目标摘要；已有标题时只写摘要；空标题回落目标文本。
  assert.equal(node.first.summaryTitle, "Parser refactor", JSON.stringify(node.first));
  assert.equal(node.first.meta.title, "Parser refactor");
  assert.equal(node.later.summaryTitle, "Quick fix", JSON.stringify(node.later));
  assert.equal(node.later.meta.title, "Warmup session");
  assert.equal(node.empty.summaryTitle, GOAL_EMPTY, JSON.stringify(node.empty));
  assert.deepEqual(rust, node);
});
