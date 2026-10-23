import assert from "node:assert/strict";
import test from "node:test";
import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import { join, dirname } from "node:path";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { randomUUID } from "node:crypto";
import { DatabaseSync } from "node:sqlite";
import { fixture, binary } from "./escode-cli-rust-fixture.js";
import { copyLegacySeed } from "./escode-cli-rust-legacy-seed.js";

function seed(f: Awaited<ReturnType<typeof fixture>>) {
  // 真实 TS store 写出的 legacy 库已冻结为仓内种子；正常跑只复制，不加载 TS storage。
  return copyLegacySeed("import-lifecycle.sessions", join(f.root, "ts.sqlite"), {
    workspace: f.cwd,
    root: f.root,
  });
}

function facts(f: Awaited<ReturnType<typeof fixture>>) {
  const db = new DatabaseSync(join(f.dataDir, "rust-sessions.sqlite"), { readOnly: true });
  try {
    return Object.fromEntries(
      ["rust_session", "rust_message", "rust_row", "rust_command", "rust_legacy_import"].map(
        (table) => [table, db.prepare(`SELECT count(*) AS n FROM ${table}`).get()!.n],
      ),
    );
  } finally {
    db.close();
  }
}

test("Failed workspace imports roll back all facts and owned files; retry and restart remain idempotent", async () => {
  const f = await fixture({ legacy: true });
  try {
    const source = await seed(f);
    const before = await readFile(source);
    for (let attempt = 0; attempt < 2; attempt++) {
      const h = f.start();
      await h.wait((m) => m.method === "startup/storageState" && m.params.phase === "failed");
      await h.close(1);
      assert.match(h.stderr, /Legacy attachment missing/);
      assert.deepEqual(facts(f), {
        rust_session: 0,
        rust_message: 0,
        rust_row: 0,
        rust_command: 0,
        rust_legacy_import: 0,
      });
      assert.deepEqual(
        (await readdir(f.dataDir)).filter((p) => /^ts-(backup|import)-/.test(p)),
        [],
      );
      assert.deepEqual(await readFile(source), before);
    }
    await writeFile(join(f.cwd, "missing.txt"), "fixed source attachment");
    const h = f.start();
    await h.subscribe("conversation/first");
    await h.subscribe("conversation/second");
    await h.close();
    const files = (await readdir(f.dataDir)).filter((p) => /^ts-(backup|import)-/.test(p));
    assert.equal(files.filter((p) => p.startsWith("ts-backup-")).length, 1);
    assert.equal(facts(f).rust_session, 2);
    const again = f.start();
    await again.subscribe("conversation/second");
    await again.close();
    assert.deepEqual(
      (await readdir(f.dataDir)).filter((p) => /^ts-(backup|import)-/.test(p)),
      files,
    );
    assert.deepEqual(await readFile(source), before);
  } finally {
    await f.close();
  }
});

test("Startup reclaims only uncommitted owned attempts, retaining committed and old backups", async () => {
  const f = await fixture({ legacy: true });
  try {
    await seed(f);
    await writeFile(join(f.cwd, "missing.txt"), "available");
    const h = f.start();
    await h.subscribe("conversation/first");
    await h.close();
    const committed = (await readdir(f.dataDir)).filter((p) => /^ts-(backup|import)-/.test(p));
    const id = randomUUID();
    const abandoned = join(f.dataDir, `ts-import-${id}`);
    await mkdir(abandoned);
    await writeFile(join(abandoned, "snapshot.sqlite"), "unfinished snapshot");
    await writeFile(join(f.dataDir, `ts-backup-${id}.sqlite`), "published before crash");
    await writeFile(join(f.dataDir, "ts-backup-old.sqlite"), "preserve old format");
    const again = f.start();
    await again.subscribe("conversation/first");
    await again.close();
    assert.deepEqual(
      (await readdir(f.dataDir)).filter((p) => /^ts-(backup|import)-/.test(p)).sort(),
      [...committed, "ts-backup-old.sqlite"].sort(),
    );
  } finally {
    await f.close();
  }
});

test("EOF and SIGTERM cancel startup import lock waits without ready or partial history", async () => {
  const f = await fixture({ legacy: true });
  try {
    await seed(f);
    await writeFile(join(f.cwd, "missing.txt"), "available");
    await mkdir(f.dataDir);
    const holder = spawn(
      join(
        dirname(binary),
        "examples",
        process.platform === "win32" ? "import_lock.exe" : "import_lock",
      ),
      [join(f.dataDir, "ts-import.lock")],
    );
    const exited = once(holder, "close");
    try {
      assert.match(String((await once(holder.stdout, "data"))[0]), /locked/);
      for (const mode of process.platform === "win32" ? ["eof"] : ["eof", "signal"]) {
        const h = f.start();
        await h.wait((m) => m.method === "startup/storageState" && m.params.phase === "checking");
        const started = Date.now();
        if (mode === "signal") {
          const watchdog = setTimeout(() => h.child.kill("SIGKILL"), 3000);
          try {
            h.child.kill("SIGTERM");
            await h.exited;
          } finally {
            clearTimeout(watchdog);
          }
        }
        await h.close();
        assert.ok(Date.now() - started < 2000, "cancellation should not wait for the held lock");
        assert.ok(
          !h.messages.some(
            (m) => m.method === "startup/storageState" && m.params.phase === "ready",
          ),
        );
        assert.deepEqual(
          (await readdir(f.dataDir)).filter((p) => /^ts-(backup|import)-/.test(p)),
          [],
        );
      }
    } finally {
      holder.stdin.end();
      await exited;
    }
    const h = f.start();
    await h.subscribe("conversation/first");
    await h.close();
    assert.equal(facts(f).rust_session, 2);
  } finally {
    await f.close();
  }
});

test("Failure at the final import marker rolls back new history and preserves existing native history", async () => {
  const f = await fixture();
  try {
    const native = f.start();
    const nativeId = await native.create("native history stays");
    await native.subscribe(`conversation/${nativeId}`);
    await native.completed(nativeId);
    await native.close();
    const source = await seed(f);
    await writeFile(join(f.cwd, "missing.txt"), "available");
    const beforeSource = await readFile(source);
    const db = new DatabaseSync(join(f.dataDir, "rust-sessions.sqlite"));
    db.exec(
      "CREATE TABLE rust_legacy_import(source TEXT NOT NULL,workspace TEXT NOT NULL,backup TEXT NOT NULL,PRIMARY KEY(source,workspace)); CREATE TRIGGER fail_import BEFORE INSERT ON rust_legacy_import BEGIN SELECT RAISE(ABORT, 'fixture import commit failure'); END;",
    );
    const before = facts(f);
    db.close();
    const h = f.start();
    await h.wait((m) => m.method === "startup/storageState" && m.params.phase === "failed");
    await h.close(1);
    assert.match(h.stderr, /fixture import commit failure/);
    assert.deepEqual(facts(f), before);
    assert.deepEqual(
      (await readdir(f.dataDir)).filter((p) => /^ts-(backup|import)-/.test(p)),
      [],
    );
    const fix = new DatabaseSync(join(f.dataDir, "rust-sessions.sqlite"));
    fix.exec("DROP TRIGGER fail_import");
    fix.close();
    const again = f.start();
    await again.subscribe(`conversation/${nativeId}`);
    assert.ok(
      (await again.rows(nativeId)).rows.some((r: any) => r.text === "native history stays"),
    );
    await again.subscribe("conversation/first");
    await again.close();
    assert.equal(facts(f).rust_session, 3);
    assert.deepEqual(await readFile(source), beforeSource);
  } finally {
    await f.close();
  }
});
