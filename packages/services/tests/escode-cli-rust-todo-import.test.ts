import assert from "node:assert/strict";
import test from "node:test";
import { copyFile, mkdir, readFile, readdir } from "node:fs/promises";
import { join, resolve } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { fixture } from "./escode-cli-rust-fixture.js";
import { escodeSessionStateSnapshotSchema } from "@escode/shared";
import { canonical, nodeOracle } from "./escode-cli-rust-golden.js";

// Phase 3：TS session store 只在录制时生成种子 SQLite；正常跑复制仓内冻结的 .sqlite 资产。
const sessionStoreModule =
  "../../../apps/escode-cli/packages/adapters/src/storage/session-store.js";
const goldenDir = resolve("packages/services/tests/fixtures/cli-golden");
const recording = process.env.ESCODE_RECORD_CLI_GOLDEN === "1";
const SEED_VALID = "todo-import.seed-valid.sqlite";
const SEED_UPDATED = "todo-import.seed-updated.sqlite";
const SEED_INVALID = "todo-import.seed-invalid.sqlite";

/** 仓内资产写入前抹掉录制机器的临时路径，避免 golden 携带本机目录。 */
function sanitizeSeed(artifact: string) {
  const db = new DatabaseSync(artifact);
  try {
    db.prepare("UPDATE session SET workspace_id=?, directory=?").run("<WORKSPACE>", "<WORKSPACE>");
    db.exec("VACUUM");
  } finally {
    db.close();
  }
}

/** 冻结的种子库带有占位工作区路径；复制后改成当前 fixture 的路径。 */
function adjustWorkspace(source: string, cwd: string) {
  const db = new DatabaseSync(source);
  try {
    const row = db.prepare("SELECT directory FROM session LIMIT 1").get() as
      | { directory?: string }
      | undefined;
    if (row?.directory && row.directory !== cwd)
      db.prepare("UPDATE session SET workspace_id=?, directory=?").run(cwd, cwd);
  } finally {
    db.close();
  }
}

async function seed(f: Awaited<ReturnType<typeof fixture>>, publish?: string) {
  const source = join(f.root, "ts.sqlite");
  if (!recording) {
    await copyFile(join(goldenDir, publish ?? SEED_VALID), source);
    adjustWorkspace(source, f.cwd);
    return source;
  }
  const { createSqliteSessionStore } = await import(sessionStoreModule);
  const store = createSqliteSessionStore({ dbPath: source });
  for (const id of ["first", "second"]) {
    await store.createSession({
      id: id as any,
      projectID: "fixture" as any,
      workspaceID: f.cwd as any,
      directory: f.cwd,
      slug: id,
      title: id,
      version: "fixture",
    });
    await store.saveSessionEntry({
      id: `${id}-mode`,
      sessionID: id as any,
      type: "runtime/execution_state",
      time: { created: 1, updated: 1 },
      data: { mode: "yolo", planEnabled: false },
    });
    await store.updateTodos({
      sessionID: id as any,
      todos: [
        { content: `${id} original`, status: "in_progress", priority: "high" },
        { content: "next", status: "pending", priority: "low" },
      ],
    });
  }
  store.close();
  if (publish) {
    await mkdir(goldenDir, { recursive: true });
    await copyFile(source, join(goldenDir, publish));
    sanitizeSeed(join(goldenDir, publish));
  }
  return source;
}

function state(f: Awaited<ReturnType<typeof fixture>>, id: string) {
  const db = new DatabaseSync(join(f.dataDir, "rust-sessions.sqlite"), { readOnly: true });
  try {
    return JSON.parse(
      db.prepare("SELECT body FROM rust_session WHERE id=?").get(id)!.body as string,
    );
  } finally {
    db.close();
  }
}

test("Rust imports authoritative frozen Todo lists and backfills missing native fields from the committed backup only", async () => {
  const f = await fixture({ legacy: true });
  try {
    const source = await seed(f, SEED_VALID);
    // TS store 读出的权威 Todo 列表冻结为观察；种子文件与观察同批录制。
    const seeded = await nodeOracle("seedTodos", async () => {
      const { createSqliteSessionStore } = await import(sessionStoreModule);
      const store = createSqliteSessionStore({ dbPath: source });
      const first = await store.readTodos({ sessionID: "first" as any });
      const second = await store.readTodos({ sessionID: "second" as any });
      store.close();
      return { first, second };
    });
    const original = await readFile(source);
    const h = f.start();
    await h.subscribe("conversation/first");
    const read = await h.client.request(
      "session/read",
      { sessionId: "first" },
      escodeSessionStateSnapshotSchema,
    );
    assert.deepEqual(canonical(read.todos), seeded.first);
    await h.close();
    assert.deepEqual(await readFile(source), original);
    const files = (await readdir(f.dataDir)).filter((name) => name.startsWith("ts-backup-"));
    assert.equal(files.length, 1);
    const backup = await readFile(join(f.dataDir, files[0]!));
    // 模拟旧版本已导入但未保存 todos；另一会话的显式清空必须保留。
    const native = new DatabaseSync(join(f.dataDir, "rust-sessions.sqlite"));
    native
      .prepare(
        "UPDATE rust_session SET body=json_remove(body,'$.todos','$.todosUpdatedAt') WHERE id='first'",
      )
      .run();
    native
      .prepare("UPDATE rust_session SET body=json_set(body,'$.todos',json('[]')) WHERE id='second'")
      .run();
    native.close();
    // TS 在种子库上继续写入新的 todos；正常跑直接换成冻结的更新后种子。
    if (!recording) {
      await copyFile(join(goldenDir, SEED_UPDATED), source);
      adjustWorkspace(source, f.cwd);
    } else {
      const { createSqliteSessionStore } = await import(sessionStoreModule);
      const current = createSqliteSessionStore({ dbPath: source });
      await current.updateTodos({
        sessionID: "first" as any,
        todos: [
          {
            content: "new TS state must not replace imported snapshot",
            status: "completed",
            priority: "low",
          },
        ],
      });
      current.close();
      await copyFile(source, join(goldenDir, SEED_UPDATED));
      sanitizeSeed(join(goldenDir, SEED_UPDATED));
    }
    const changedSource = await readFile(source);
    for (let i = 0; i < 2; i++) {
      const cold = f.start();
      await cold.subscribe("conversation/first");
      const first = await cold.client.request(
        "session/read",
        { sessionId: "first" },
        escodeSessionStateSnapshotSchema,
      );
      assert.deepEqual(canonical(first.todos), seeded.first);
      await cold.close();
      assert.deepEqual(canonical(state(f, "second").todos), []);
      assert.deepEqual(await readFile(source), changedSource);
      assert.deepEqual(await readFile(join(f.dataDir, files[0]!)), backup);
      assert.deepEqual(
        (await readdir(f.dataDir)).filter((name) => name.startsWith("ts-backup-")),
        files,
      );
    }
  } finally {
    await f.close();
  }
});

test("Rust rejects invalid stored TS Todo state and rolls back every imported session", async () => {
  const f = await fixture({ legacy: true });
  try {
    const source = join(f.root, "ts.sqlite");
    if (recording) {
      // 先按有效种子生成，再注入空 content 并冻结为无效种子。
      const valid = await seed(f);
      const db = new DatabaseSync(valid);
      db.prepare("UPDATE todo SET content='' WHERE session_id='second'").run();
      db.close();
      await copyFile(valid, join(goldenDir, SEED_INVALID));
      sanitizeSeed(join(goldenDir, SEED_INVALID));
    } else {
      await copyFile(join(goldenDir, SEED_INVALID), source);
      adjustWorkspace(source, f.cwd);
    }
    const original = await readFile(source);
    const h = f.start();
    await h.wait((m) => m.method === "startup/storageState" && m.params.phase === "failed");
    await h.close(1);
    assert.match(h.stderr, /Todo content must not be empty/);
    const native = new DatabaseSync(join(f.dataDir, "rust-sessions.sqlite"), { readOnly: true });
    assert.equal(native.prepare("SELECT count(*) AS n FROM rust_session").get()!.n, 0);
    assert.equal(native.prepare("SELECT count(*) AS n FROM rust_legacy_import").get()!.n, 0);
    native.close();
    assert.deepEqual(await readFile(source), original);
    assert.deepEqual(
      (await readdir(f.dataDir)).filter((p) => /^ts-(backup|import)-/.test(p)),
      [],
    );
  } finally {
    await f.close();
  }
});
