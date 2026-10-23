import assert from "node:assert/strict";
import test from "node:test";
import { readFile, readdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { fixture } from "./escode-cli-rust-fixture.js";
import { copyLegacySeed } from "./escode-cli-rust-legacy-seed.js";
import { escodeSessionStateSnapshotSchema } from "@escode/shared";

test("Real TS storage imports identity, attachments, tools and interrupted outcomes once; source remains usable for rollback", async () => {
  const f = await fixture({ legacy: true });
  try {
    const path = join(f.root, "ts.sqlite");
    const id = "ts-session";
    // 真实 TS store 写出的库已冻结为仓内种子；附件仍由测试写入当前临时 root。
    await copyLegacySeed("migration.real", path, { workspace: f.cwd, root: f.root });
    const attachment = join(f.root, "old.txt");
    await writeFile(attachment, "old attachment content");
    const before = await readFile(path);
    const h = f.start();
    await h.subscribe(`conversation/${id}`);
    const snapshot = await h.client.request(
      "session/read",
      { sessionId: id },
      escodeSessionStateSnapshotSchema,
    );
    assert.equal(snapshot.session.title, "Old TS task");
    assert.ok(
      snapshot.messages
        .flatMap((m) => m.parts)
        .some((p) => p.type === "file" && p.filename === "old.txt" && p.mime === "text/plain"),
    );
    assert.ok(
      snapshot.messages
        .flatMap((m) => m.parts)
        .some((p) => p.type === "tool" && p.callId === "old-write" && p.state.status === "error"),
    );
    const rows = await h.rows(id);
    assert.ok(
      rows.rows.some(
        (r: any) => r.kind === "userInput" && r.attachments?.[0].fileName === "old.txt",
      ),
    );
    assert.ok(
      rows.rows.some(
        (r: any) =>
          r.kind === "toolCall" && r.toolCallId === "old-write" && r.status === "cancelled",
      ),
    );
    await h.command(h.envelope("sendText", id, { text: "continue old task" }));
    await h.completed(id);
    assert.match(JSON.stringify(f.requests[0]!.messages), /old attachment content/);
    assert.match(JSON.stringify(f.requests[0]!.messages), /execution outcome is unknown/);
    assert.deepEqual(
      f.requests[0]!.messages.filter((m: any) => m.role === "tool").map((m: any) => m.tool_call_id),
      ["old-read", "old-write"],
    );
    await h.close();
    const h2 = f.start();
    await h2.subscribe(`conversation/${id}`);
    const restored = await h2.rows(id);
    assert.equal(restored.rows.filter((r: any) => r.entityId === "ts-user").length, 2);
    await h2.close();
    assert.deepEqual(await readFile(path), before);
    const files = await readdir(f.dataDir);
    assert.equal(
      files.filter((p) => p.startsWith("ts-backup-") && p.endsWith(".sqlite")).length,
      1,
    );
    const source = new DatabaseSync(path, { readOnly: true });
    assert.equal((source.prepare("SELECT count(*) AS n FROM message").get() as { n: number }).n, 2);
    source.close();
    assert.deepEqual(h.schemaErrors, []);
    assert.deepEqual(h2.schemaErrors, []);
  } finally {
    await f.close();
  }
});

test("TS migration preserves workspace identity, runs imported build sessions and retains discarded input ACKs", async () => {
  const f = await fixture({ legacy: true });
  try {
    // 本地/远程 workspace 与排队输入的最终形态已由 TS store 冻结为仓内种子。
    await copyLegacySeed("migration.workspaces", join(f.root, "ts.sqlite"), {
      workspace: f.cwd,
      root: f.root,
    });
    const local = f.start();
    await local.subscribe("conversation/local");
    await assert.rejects(local.rows("remote"), /Session unavailable/);
    // 导入的 build 会话现在按 build 规则执行（写入需确认），不再被拒；
    // 但旧存储里排队、重启时丢弃的输入绝不能被执行。
    const accepted = await local.command(
      local.envelope("sendText", "local", { text: "no elevation" }),
    );
    assert.equal(accepted.status, "accepted");
    await local.completed("local");
    assert(
      !f.requests.some((r: any) => JSON.stringify(r.messages ?? r).includes("must not execute")),
    );
    const duplicate = await local.command({
      ...local.envelope("sendText", "local", { text: "must not execute" }),
      commandId: "local-command",
    });
    assert.equal(duplicate.status, "failed");
    assert.equal(duplicate.reasonCode, "fault.input.discardedOnRestart");
    await local.command(local.envelope("switchCollaborationMode", "local", { mode: "yolo" }));
    await local.command(local.envelope("sendText", "local", { text: "explicit yolo" }));
    await local.completed("local");
    const remote = f.start("ssh://fixture/workspace");
    await remote.subscribe("conversation/remote");
    await assert.rejects(remote.rows("local"), /Session unavailable/);
    assert.deepEqual(local.schemaErrors, []);
    assert.deepEqual(remote.schemaErrors, []);
  } finally {
    await f.close();
  }
});

test("TS compact summary restores its preserved tail without resurrecting summarized history", async () => {
  const f = await fixture({ legacy: true });
  try {
    // compact 摘要与保留尾部的最终形态已由 TS store 冻结为仓内种子。
    await copyLegacySeed("migration.compact", join(f.root, "ts.sqlite"), {
      workspace: f.cwd,
      root: f.root,
    });
    const id = "compact-ts";
    const h = f.start();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "continue" }));
    await h.completed(id);
    const request = JSON.stringify(f.requests[0]!.messages);
    assert.match(request, /Summary from TS/);
    assert.match(request, /preserved tail/);
    assert.doesNotMatch(request, /summarized text/);
    assert.deepEqual(h.schemaErrors, []);
  } finally {
    await f.close();
  }
});
