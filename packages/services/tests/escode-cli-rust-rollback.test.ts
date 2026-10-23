import assert from "node:assert/strict";
import test from "node:test";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { fixture } from "./escode-cli-rust-fixture.js";
import { copyLegacySeed } from "./escode-cli-rust-legacy-seed.js";

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
    const id = "rollback-session";
    // 真实 TS store 写出的库已冻结为仓内种子；正常跑不再加载 TS storage。
    await copyLegacySeed("rollback.session", tsDb, { workspace: f.cwd, root: f.root });
    const before = await digest(tsDb);

    // 第一次：Rust 导入并在自己的存储里跑一轮会话。
    const first = f.start();
    await first.subscribe(`conversation/${id}`);
    await first.command(first.envelope("sendText", id, { text: "hello" }));
    await first.completed(id);
    await first.close();
    assert.equal(await digest(tsDb), before, "Rust import mutated the TS source database");

    // 源库仍按 TS schema 可读（回退后 Node 继续用同一份数据）：直接读原始 SQLite，
    // 不引入 TS storage 实现。
    const reopened = new DatabaseSync(tsDb, { readOnly: true });
    try {
      const row = reopened
        .prepare("SELECT id FROM session WHERE COALESCE(NULLIF(TRIM(workspace_id),''),directory)=?")
        .get(f.cwd) as { id: string } | undefined;
      assert.equal(row?.id, id);
    } finally {
      reopened.close();
    }

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
    const db = new DatabaseSync(join(f.dataDir, "rust-sessions.sqlite"), { readOnly: true });
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
