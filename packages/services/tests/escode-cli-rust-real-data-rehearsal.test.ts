import assert from "node:assert/strict";
import test from "node:test";
import { createHash } from "node:crypto";
import { cp, mkdir, mkdtemp, readFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { DatabaseSync, backup } from "node:sqlite";
import { fixture } from "./escode-cli-rust-fixture.js";
import { projectRows } from "./escode-cli-rust-row-projection.js";

// 数据迁移验收（docs/specs/rust-import-lifecycle.md「真实数据演练」）：对真实 TS 数据的只读演练。
// 默认跳过；设置 ESCODE_REHEARSAL_DB=<~/.escode/cli/db/db.sqlite> 后运行：
// - 用 SQLite backup 取一致副本，原库只读、结束时校验哈希不变；
// - 附件目录取 ESCODE_REHEARSAL_ARTIFACTS（缺省为数据库上两级的 artifacts）并复制；
// - Node runtime 与 Rust 导入各自打开独立副本，逐会话比较行种类计数、附件数与 todo 数；
// - 只输出计数，不输出任何会话内容。
const source = process.env.ESCODE_REHEARSAL_DB;
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");
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

/** 行内容哈希：归一化 id/时间戳/路径后对整份行序列取 SHA-256，避免只比计数。 */
const rowHash = (projected: Rows) =>
  createHash("sha256").update(JSON.stringify(projected)).digest("hex");

/** 首个差异 JSON 路径：只用于定位字段，不输出字段值（真实数据演练不得打印会话内容）。 */
function firstDifferencePath(a: unknown, b: unknown, path = "$"): string | null {
  if (Array.isArray(a) && Array.isArray(b)) {
    if (a.length !== b.length) return `${path}.length`;
    for (let index = 0; index < a.length; index++) {
      const found = firstDifferencePath(a[index], b[index], `${path}[${index}]`);
      if (found) return found;
    }
    return null;
  }
  if (a && b && typeof a === "object" && typeof b === "object") {
    const keys = [...new Set([...Object.keys(a), ...Object.keys(b)])].sort();
    for (const key of keys) {
      const found = firstDifferencePath(
        (a as Record<string, unknown>)[key],
        (b as Record<string, unknown>)[key],
        `${path}.${key}`,
      );
      if (found) return found;
    }
    return null;
  }
  return Object.is(a, b) ? null : path;
}

async function copyOf(label: string) {
  const root = await mkdtemp(join(tmpdir(), `escode-rehearsal-${label}-`));
  const src = new DatabaseSync(source!, { readOnly: true });
  try {
    await backup(src, join(root, "ts.sqlite"));
  } finally {
    src.close();
  }
  const artifacts =
    process.env.ESCODE_REHEARSAL_ARTIFACTS ?? join(dirname(dirname(source!)), "artifacts");
  if (existsSync(artifacts)) {
    await mkdir(join(root, ".escode", "cli"), { recursive: true });
    await cp(artifacts, join(root, ".escode", "cli", "artifacts"), { recursive: true });
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
  { skip: !source && "set ESCODE_REHEARSAL_DB to run the real-data rehearsal" },
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
      const roots = [s.ws, nodeRoot, rustRoot];
      const rustRows = projectRows(r.rows, roots);
      const nodeRows = projectRows(n.rows, roots);
      const rowDiffPath = firstDifferencePath(rustRows, nodeRows);
      return {
        session: index + 1,
        sessionId: s.id,
        rowsMatch: JSON.stringify(kinds(r.rows)) === JSON.stringify(kinds(n.rows)),
        rowsHashMatch: rowDiffPath === null,
        rowHashes: [rowHash(rustRows), rowHash(nodeRows)],
        rowDiffPath,
        rust: kinds(r.rows),
        node: kinds(n.rows),
        files: [files(r.rows), files(n.rows)],
        todos: [r.todos, n.todos],
      };
    });
    console.log("REHEARSAL", JSON.stringify(report));
    if (process.env.DEBUG_REHEARSAL) {
      // 只打印结构与枚举字段，绝不输出会话文本/路径（真实数据演练约束）。
      for (const row of report) {
        const index = Number(/\[(\d+)\]/.exec(row.rowDiffPath ?? "")?.[1]);
        if (!row.rowDiffPath || Number.isNaN(index)) continue;
        const session = sessions.find((candidate) => candidate.id === row.sessionId)!;
        const rustRows = projectRows(rust.get(row.sessionId)!.rows, [
          session.ws,
          nodeRoot,
          rustRoot,
        ]);
        const nodeRows = projectRows(node.get(row.sessionId)!.rows, [
          session.ws,
          nodeRoot,
          rustRoot,
        ]);
        const pick = (value: Record<string, any> | undefined) =>
          value
            ? {
                kind: value.kind,
                actions: value.actions,
                state: value.state,
                origin: value.origin,
                executionKind: value.executionKind,
                historyRoundCount: value.historyRoundCount,
                outputTokens: value.outputTokens,
                activeMs: value.activeMs,
                hasOutput: value.output !== undefined,
                keys: Object.keys(value).sort(),
              }
            : null;
        console.log(
          "REHEARSAL-ROW",
          JSON.stringify({
            session: row.session,
            index,
            path: row.rowDiffPath,
            rust: pick(rustRows[index]),
            node: pick(nodeRows[index]),
            rustActions: rustRows.map((row) => [row.kind, row.actions ?? null]),
            nodeActions: nodeRows.map((row) => [row.kind, row.actions ?? null]),
          }),
        );
      }
    }
    assert.equal(
      await sha(join(rustRoot, "ts.sqlite")),
      copyBefore,
      "import must not modify its source",
    );
    assert.equal(await sha(source!), before, "the original database must stay untouched");
    for (const row of report) {
      assert(row.rowsMatch, `session ${row.session}: row kinds differ`);
      assert(
        row.rowsHashMatch,
        `session ${row.session}: row content differs at ${row.rowDiffPath ?? "unknown"}`,
      );
      assert.equal(row.files[0], row.files[1], `session ${row.session}: attachments differ`);
      assert.equal(row.todos[0], row.todos[1], `session ${row.session}: todos differ`);
    }
  },
);
