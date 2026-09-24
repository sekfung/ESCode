import assert from "node:assert/strict";
import test from "node:test";
import { DatabaseSync } from "node:sqlite";
import { join, delimiter } from "node:path";
import { mkdir, writeFile, access, symlink } from "node:fs/promises";
import { fixture } from "./zcode-cli-rust-fixture.js";

test("Metadata-only commits do not rewrite unchanged transcript or history boundaries", async () => {
  const f = await fixture();
  try {
    const h = f.start(),
      id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "seed" }));
    await h.completed(id);
    const db = new DatabaseSync(join(f.dataDir, "rust-sessions.sqlite"));
    for (const table of ["rust_row", "rust_history"])
      db.exec(
        `CREATE TRIGGER no_rewrite_${table} BEFORE UPDATE ON ${table} WHEN new.body=old.body BEGIN SELECT RAISE(ABORT,'unchanged fact rewritten'); END;`,
      );
    assert.equal(
      (await h.command(h.envelope("renameSession", id, { title: "renamed" }))).status,
      "accepted",
    );
    assert.equal((await h.rows(id)).rows.filter((r) => r.kind === "userInput").length, 1);
    db.close();
    assert.deepEqual(h.schemaErrors, []);
  } finally {
    await f.close();
  }
});

test("A single oversized idle session is evicted by bytes even below the count limit", async () => {
  const f = await fixture();
  try {
    let h = f.start();
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "seed" }));
    await h.completed(id);
    await h.close();
    const db = new DatabaseSync(join(f.dataDir, "rust-sessions.sqlite"));
    const insert = db.prepare(
      "INSERT INTO rust_message SELECT workspace,session,?,? FROM rust_message WHERE session=? LIMIT 1",
    );
    db.exec("BEGIN");
    for (let i = 0; i < 350; i++)
      insert.run(1000 + i, JSON.stringify({ role: "user", content: "x".repeat(64 * 1024) }), id);
    db.exec("COMMIT");
    db.close();
    h = f.start();
    const first = await h.rows(id),
      second = await h.rows(id);
    assert.notEqual(first.atLogEpoch, second.atLogEpoch);
    await h.subscribe(`conversation/${id}`);
    const pinned = (await h.rows(id)).atLogEpoch;
    assert.equal((await h.rows(id)).atLogEpoch, pinned);
    await h.client.request("v4/connection/flow", {
      connectionId: "fixture-desktop",
      state: "closed",
    });
    assert.notEqual((await h.rows(id)).atLogEpoch, pinned);
    assert.equal(f.requests.length, 1);
    assert.deepEqual(h.schemaErrors, []);
  } finally {
    await f.close();
  }
});

test("Non-repository context skips Git spawn but explicit GIT_DIR still invokes discovery", async () => {
  // nested-symlink 需要创建目录符号链接，Windows 上通常需要开发者模式/管理员权限，保留在 POSIX 验证。
  const kinds =
    process.platform === "win32"
      ? ["absent", "explicit"]
      : ["absent", "explicit", "nested-symlink"];
  for (const kind of kinds) {
    const env: Record<string, string> = {};
    const f = await fixture({ env });
    try {
      const marker = join(f.cwd, "git-invoked");
      // 用 git 自身的 trace 输出判断是否被调用：Windows 上无法用无扩展名的 shell 脚本伪造 git，
      // 而 git.exe 会遵守 GIT_TRACE2_EVENT，两端语义一致。
      env.GIT_TRACE2_EVENT = marker;
      if (kind === "explicit") env.GIT_DIR = join(f.cwd, "explicit-repo");
      if (kind === "nested-symlink") {
        const physical = join(f.root, "repo");
        await mkdir(join(physical, "nested"), { recursive: true });
        await writeFile(join(physical, ".git"), "gitdir: fixture");
        // Git 按物理 cwd 查找父目录；符号链接路径不能造成错误的非仓库判定。
        const { rm } = await import("node:fs/promises");
        await rm(f.cwd, { recursive: true });
        await symlink(join(physical, "nested"), f.cwd, "dir");
      }
      const h = f.start(),
        id = await h.create();
      await h.subscribe(`conversation/${id}`);
      await h.command(h.envelope("sendText", id, { text: "seed" }));
      await h.completed(id);
      if (kind !== "absent") await access(marker);
      else await assert.rejects(access(marker));
      assert.deepEqual(h.schemaErrors, []);
    } finally {
      await f.close();
    }
  }
});
