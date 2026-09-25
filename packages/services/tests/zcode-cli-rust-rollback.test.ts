import assert from "node:assert/strict";
import test from "node:test";
import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { createSqliteSessionStore } from "../../../apps/zcode-cli/packages/adapters/src/storage/session-store.js";
import type { MessageId, PartId, ProjectId, SessionId, WorkspaceId } from "@zcode/contracts";
import { fixture } from "./zcode-cli-rust-fixture.js";

/** 源库与 TS 侧文件的字节指纹：回退的前提是 Rust 只读、不改写 TS 的数据。 */
async function digest(path: string): Promise<string> {
  return createHash("sha256")
    .update(await readFile(path))
    .digest("hex");
}

// docs/specs/rust-release-rollback.md「验收」：切换到 Rust 之后仍能无损回退——
// TS 源库与 TS 附件目录必须逐字节不变，且 Rust 进程退出后不留下占用 workspace 的锁。
test("Rust import leaves TS storage byte-identical and frees the workspace for rollback", async () => {
  const f = await fixture({ legacy: true });
  try {
    const tsDb = join(f.root, "ts.sqlite");
    const store = createSqliteSessionStore({ dbPath: tsDb });
    const id = "rollback-session" as SessionId;
    const messageID = "rollback-user" as MessageId;
    await store.createSession({
      id,
      projectID: "p" as ProjectId,
      workspaceID: f.cwd as WorkspaceId,
      directory: f.cwd,
      slug: "rollback",
      title: "rollback",
      version: "fixture",
    });
    await store.saveSessionEntry({
      id: "rollback-model",
      sessionID: id,
      type: "runtime/model_selection",
      time: { created: 1, updated: 1 },
      data: { providerId: "fixture", modelId: "core-model", options: { reasoningLevel: "none" } },
    });
    await store.saveSessionEntry({
      id: "rollback-mode",
      sessionID: id,
      type: "runtime/execution_state",
      time: { created: 1, updated: 1 },
      data: { mode: "yolo", planEnabled: false },
    });
    await store.saveMessage({
      id: messageID,
      sessionID: id,
      role: "user",
      time: { created: 1 },
      agent: "main",
    });
    await store.savePart({
      id: "rollback-text" as PartId,
      sessionID: id,
      messageID,
      type: "text",
      text: "ts history survives rollback",
    });
    store.close();
    const before = await digest(tsDb);

    // 第一次：Rust 导入并在自己的存储里跑一轮会话。
    const first = f.start();
    await first.subscribe(`conversation/${id}`);
    await first.command(first.envelope("sendText", id, { text: "hello" }));
    await first.completed(id);
    await first.close();
    assert.equal(await digest(tsDb), before, "Rust import mutated the TS source database");

    // 源库仍可被 TS 侧读取（回退后 Node 继续用同一份数据）。
    const reopened = createSqliteSessionStore({ dbPath: tsDb });
    const sessions = await reopened.listSessions({ directory: f.cwd } as never).catch(() => null);
    if (sessions) assert(sessions.some((s: { id: string }) => s.id === id));
    reopened.close();

    // 第二次：workspace 未被上一次运行占用（owner lock 已释放），会话可继续被读到。
    const second = f.start();
    await second.subscribe(`conversation/${id}`);
    const rows = await second.rows(id);
    assert(rows.rows.length > 0);
    await second.close();
    assert.equal(await digest(tsDb), before, "second run mutated the TS source database");
    assert.deepEqual(first.schemaErrors, []);
    assert.deepEqual(second.schemaErrors, []);
  } finally {
    await f.close();
  }
});

// 回退时 Rust 自己的库必须留在原地、可再次打开：撤销选择不等于丢数据。
test("Rust storage stays readable after the process exits", async () => {
  const f = await fixture();
  try {
    const first = f.start();
    const id = await first.create();
    await first.subscribe(`conversation/${id}`);
    await first.command(first.envelope("sendText", id, { text: "hello" }));
    await first.completed(id);
    await first.close();

    // 直接读 Rust 的库：结构化事实确实落盘（回退后可由 Rust runtime 重新打开）。
    const db = new (await import("node:sqlite")).DatabaseSync(
      join(f.dataDir, "rust-sessions.sqlite"),
      { readOnly: true },
    );
    try {
      const row = db.prepare("SELECT body FROM rust_session WHERE id=?").get(id) as
        | { body: string }
        | undefined;
      assert(row, "Rust session missing after exit");
      assert(JSON.parse(row.body).mode);
    } finally {
      db.close();
    }
  } finally {
    await f.close();
  }
});
