import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";
import { access } from "node:fs/promises";
import { join } from "node:path";
import { fixture, event, end, type Harness } from "./zcode-cli-rust-fixture.js";

// docs/specs/rust-permission-modes.md「验收 3」：build 模式写文件必须经用户确认，
// 允许才执行、拒绝不执行且回 TS 拒绝文案；「总是允许」写入项目规则并在后续会话生效。
type Message = Record<string, any>;

function writeCall(res: any, id: string) {
  event(res, {
    tool_calls: [
      {
        index: 0,
        id,
        type: "function",
        function: {
          name: "Write",
          arguments: JSON.stringify({ file_path: "out.txt", content: "written" }),
        },
      },
    ],
  });
  end(res, "tool_calls");
}

async function pendingPermission(h: Harness, id: string, after = 0) {
  const m = await h.wait(
    (m) =>
      m.params?.topic === `conversation/${id}` &&
      m.params.frame?.payload?.deltas?.some((d: Message) =>
        d.patch?.pendingInteractions?.some((p: Message) => p.kind === "permission"),
      ),
    after,
  );
  return m.params.frame.payload.deltas
    .flatMap((d: Message) => d.patch?.pendingInteractions ?? [])
    .find((p: Message) => p.kind === "permission");
}

function writeFixture() {
  let step = 0;
  return fixture({
    respond(_req, res) {
      res.writeHead(200, { "content-type": "text/event-stream" });
      if (step++ === 0) writeCall(res, "w1");
      else {
        event(res, { content: "done" });
        end(res, "stop");
      }
    },
  });
}

async function exists(path: string) {
  return access(path).then(
    () => true,
    () => false,
  );
}

test("Rust build mode asks before Write and runs it after Allow once", async () => {
  const f = await writeFixture();
  try {
    const h = f.start();
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "write", mode: "build" }));
    const p = await pendingPermission(h, id);
    assert.equal(p.payload.toolName, "Write");
    assert.equal(p.payload.freeText, true);
    assert.deepEqual(
      p.payload.options.map((o: Message) => o.optionId),
      ["allowOnce", "allowAlways", "deny"],
    );
    assert.equal(await exists(join(f.cwd, "out.txt")), false);
    assert.equal(
      (
        await h.command(
          h.envelope("resolveInteraction", id, {
            interactionId: p.interactionId,
            answer: { optionId: "allowOnce" },
          }),
        )
      ).status,
      "accepted",
    );
    await h.completed(id);
    assert.equal(await readFile(join(f.cwd, "out.txt"), "utf8"), "written");
    const row = (await h.rows(id)).rows.find(
      (r: Message) => r.kind === "toolCall" && r.toolName === "Write",
    );
    assert.equal(row.status, "success");
    assert.deepEqual(h.schemaErrors, []);
  } finally {
    await f.close();
  }
});

test("Rust build mode denial keeps the file unwritten and returns the TS denial text", async () => {
  const f = await writeFixture();
  try {
    const h = f.start();
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "write", mode: "build" }));
    const p = await pendingPermission(h, id);
    await h.command(
      h.envelope("resolveInteraction", id, {
        interactionId: p.interactionId,
        answer: { optionId: "deny", freeText: "use another name" },
      }),
    );
    await h.completed(id);
    assert.equal(await exists(join(f.cwd, "out.txt")), false);
    const output = f.requests[1]!.messages.at(-1).content;
    assert.match(output, /The user doesn't want to proceed with this tool use/);
    assert.match(output, /the user said:\nuse another name/);
    assert.doesNotMatch(output, /Tool failed/);
    const row = (await h.rows(id)).rows.find(
      (r: Message) => r.kind === "toolCall" && r.toolName === "Write",
    );
    assert.equal(row.status, "error");
    assert.deepEqual(h.schemaErrors, []);
  } finally {
    await f.close();
  }
});

test("Rust yolo mode never prompts and writes directly", async () => {
  const f = await writeFixture();
  try {
    const h = f.start();
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "write", mode: "yolo" }));
    await h.completed(id);
    assert.equal(await readFile(join(f.cwd, "out.txt"), "utf8"), "written");
    assert.deepEqual(h.schemaErrors, []);
  } finally {
    await f.close();
  }
});

test("Rust always-allow writes a project rule that later sessions reuse without prompting", async () => {
  const f = await writeFixture();
  try {
    const h = f.start();
    const first = await h.create();
    await h.subscribe(`conversation/${first}`);
    await h.command(h.envelope("sendText", first, { text: "write", mode: "build" }));
    const p = await pendingPermission(h, first);
    assert.equal(
      (
        await h.command(
          h.envelope("resolveInteraction", first, {
            interactionId: p.interactionId,
            answer: { optionId: "allowAlways" },
          }),
        )
      ).status,
      "accepted",
    );
    await h.completed(first);
    assert.equal(await readFile(join(f.cwd, "out.txt"), "utf8"), "written");
    // 第二个会话同一 workspace：项目规则生效，不再弹确认。
    const second = await h.create();
    await h.subscribe(`conversation/${second}`);
    await h.command(h.envelope("sendText", second, { text: "write", mode: "build" }));
    await h.completed(second);
    assert.equal(await exists(join(f.cwd, "out.txt")), true);
    assert.deepEqual(h.schemaErrors, []);
  } finally {
    await f.close();
  }
});
