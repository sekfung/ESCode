import assert from "node:assert/strict";
import test from "node:test";
import { createHash } from "node:crypto";
import { cp, mkdir, mkdtemp, readFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { DatabaseSync, backup } from "node:sqlite";
import { fixture } from "./zcode-cli-rust-fixture.js";

// 数据迁移验收（docs/specs/rust-import-lifecycle.md「真实数据演练」）：对真实 TS 数据的只读演练。
// 默认跳过；设置 ZCODE_REHEARSAL_DB=<~/.zcode/cli/db/db.sqlite> 后运行：
// - 用 SQLite backup 取一致副本，原库只读、结束时校验哈希不变；
// - 附件目录取 ZCODE_REHEARSAL_ARTIFACTS（缺省为数据库上两级的 artifacts）并复制；
// - Node runtime 与 Rust 导入各自打开独立副本，逐会话比较行种类计数、附件数与 todo 数；
// - 只输出计数，不输出任何会话内容。
const source = process.env.ZCODE_REHEARSAL_DB;
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const sha = async (p: string) =>
  createHash("sha256")
    .update(await readFile(p))
    .digest("hex");

type Rows = Record<string, any>[];
const kinds = (rows: Rows) =>
  Object.fromEntries(
    [...new Set(rows.map((r) => r.kind))]
      .sort()
      .map((k) => [k, rows.filter((r) => r.kind === k).length]),
  );
const files = (rows: Rows) =>
  rows.filter((r) => r.kind === "userInput").reduce((n, r) => n + (r.attachments?.length ?? 0), 0);

async function copyOf(label: string) {
  const root = await mkdtemp(join(tmpdir(), `zcode-rehearsal-${label}-`));
  const src = new DatabaseSync(source!, { readOnly: true });
  try {
    await backup(src, join(root, "ts.sqlite"));
  } finally {
    src.close();
  }
  const artifacts =
    process.env.ZCODE_REHEARSAL_ARTIFACTS ?? join(dirname(dirname(source!)), "artifacts");
  if (existsSync(artifacts)) {
    await mkdir(join(root, ".zcode", "cli"), { recursive: true });
    await cp(artifacts, join(root, ".zcode", "cli", "artifacts"), { recursive: true });
  }
  return root;
}

async function open(
  kind: "node" | "rust",
  root: string,
  workspaces: string[],
  ids: Map<string, string[]>,
) {
  const out = new Map<string, { rows: Rows; todos: number | null }>();
  for (const ws of workspaces) {
    const f =
      kind === "node"
        ? await fixture({
            root,
            command: process.execPath,
            args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          })
        : await fixture({ root, legacy: true });
    try {
      const h = f.start(ws);
      for (const id of ids.get(ws) ?? []) {
        await h.subscribe(`conversation/${id}`);
        const rows = (await h.rows(id)).rows as Rows;
        const snapshot = (await h.client.request("session/read", { sessionId: id }, {
          parse: (v: unknown) => v,
        } as any)) as any;
        out.set(id, { rows, todos: snapshot?.todos?.length ?? null });
      }
      assert.deepEqual(h.schemaErrors, [], `${kind} schema errors`);
      await h.close();
    } finally {
      await f.close();
    }
  }
  return out;
}

test(
  "real-data read-only migration rehearsal: Rust import matches Node per session",
  { skip: !source && "set ZCODE_REHEARSAL_DB to run the real-data rehearsal" },
  async () => {
    const before = await sha(source!);
    const rustRoot = await copyOf("rust");
    const nodeRoot = await copyOf("node");
    const copyBefore = await sha(join(rustRoot, "ts.sqlite"));
    const db = new DatabaseSync(join(rustRoot, "ts.sqlite"), { readOnly: true });
    const sessions = db
      .prepare("select id, COALESCE(NULLIF(TRIM(workspace_id),''),directory) ws from session")
      .all() as { id: string; ws: string }[];
    db.close();
    const ids = new Map<string, string[]>();
    for (const s of sessions) ids.set(s.ws, [...(ids.get(s.ws) ?? []), s.id]);
    const workspaces = [...ids.keys()];
    const node = await open("node", nodeRoot, workspaces, ids);
    const rust = await open("rust", rustRoot, workspaces, ids);
    const report = sessions.map((s, index) => {
      const n = node.get(s.id)!;
      const r = rust.get(s.id)!;
      return {
        session: index + 1,
        rowsMatch: JSON.stringify(kinds(r.rows)) === JSON.stringify(kinds(n.rows)),
        rust: kinds(r.rows),
        node: kinds(n.rows),
        files: [files(r.rows), files(n.rows)],
        todos: [r.todos, n.todos],
      };
    });
    console.log("REHEARSAL", JSON.stringify(report));
    assert.equal(
      await sha(join(rustRoot, "ts.sqlite")),
      copyBefore,
      "import must not modify its source",
    );
    assert.equal(await sha(source!), before, "the original database must stay untouched");
    for (const row of report) {
      assert(row.rowsMatch, `session ${row.session}: row kinds differ`);
      assert.equal(row.files[0], row.files[1], `session ${row.session}: attachments differ`);
      assert.equal(row.todos[0], row.todos[1], `session ${row.session}: todos differ`);
    }
  },
);
