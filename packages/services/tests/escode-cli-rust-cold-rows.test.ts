import assert from "node:assert/strict";
import test from "node:test";
import { mkdir, mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { fixture } from "./escode-cli-rust-fixture.js";
import { canonical, goldenOracle } from "./escode-cli-rust-golden.js";
import { copyLegacySeed } from "./escode-cli-rust-legacy-seed.js";
import { projectRows } from "./escode-cli-rust-row-projection.js";

// docs/specs/rust-import-lifecycle.md / rust-row-projection.md：冷导入的最终行态必须与
// Node 冷恢复逐行一致。仓内种子覆盖工具/正文交错、actions、createdAtSeq、被中断的工具行、
// compact 摘要与相对/内联附件；Node 侧整份投影录成 golden，正常跑只启动 Rust。
const pluginHostBundle = resolve("apps/escode-cli/packages/cli/dist/plugin-host.cjs");
type Row = Record<string, any>;
type Observation = { seed: string; sessions: { id: string; rows: Row[] }[] }[];

const seeds: { name: string; extra?: "old.txt" }[] = [
  { name: "migration.real", extra: "old.txt" },
  { name: "migration.compact" },
  { name: "migration.workspaces" },
  { name: "migration-shapes.shapes" },
  { name: "session-context-differential.legacy" },
  { name: "history-request-differential.legacy" },
  { name: "import-lifecycle.sessions" },
  { name: "migration-boundaries.membership" },
];

async function groupsOf(seed: string, root: string, workspace: string) {
  const source = join(root, "ts.sqlite");
  await copyLegacySeed(seed, source, { workspace, root });
  const db = new DatabaseSync(source, { readOnly: true });
  try {
    const rows = db
      .prepare(
        "select id, COALESCE(NULLIF(TRIM(workspace_id),''),directory) ws from session where time_archived is null",
      )
      .all() as { id: string; ws: string }[];
    const groups = new Map<string, string[]>();
    for (const row of rows) groups.set(row.ws, [...(groups.get(row.ws) ?? []), row.id]);
    return groups;
  } finally {
    db.close();
  }
}

async function observe(kind: "node" | "rust"): Promise<Observation> {
  const result: Observation = [];
  for (const seed of seeds) {
    const workspace = await mkdtemp(join(tmpdir(), "cold-ws-"));
    const root = await mkdtemp(join(tmpdir(), `cold-${kind}-`));
    if (seed.extra) await writeFile(join(root, seed.extra), "old attachment content");
    // 相对附件 URL 由 runtime cwd 解析；fixture 的 cwd 固定为 <root>/workspace。
    if (seed.name === "import-lifecycle.sessions") {
      await mkdir(join(root, "workspace"), { recursive: true });
      await writeFile(join(root, "workspace", "missing.txt"), "fixed source attachment");
    }
    const groups = await groupsOf(seed.name, root, workspace);
    const sessions: Observation[number]["sessions"] = [];
    for (const [ws, ids] of groups) {
      const f =
        kind === "node"
          ? await fixture({
              root,
              command: process.execPath,
              args: ({ cwd }) => [pluginHostBundle, "app-server", "--stdio", "--cwd", cwd],
            })
          : await fixture({ root, legacy: true });
      try {
        const h = f.start(ws);
        for (const id of ids) {
          // 归档会话在 Node 侧不可 resume；rowsRange 不要求 conversation 订阅，
          // 与 migration-boundaries 一样直接读取，避免把订阅可用性混进行态对拍。
          sessions.push({
            id,
            rows: projectRows((await h.rows(id)).rows as Row[], [ws, root]),
          });
        }
        await h.close();
      } finally {
        await f.close();
      }
    }
    result.push({ seed: seed.name, sessions });
  }
  return result;
}

test(
  "cold import matches Node row-for-row across frozen legacy seeds",
  { timeout: 300000 },
  async () => {
    const node = await goldenOracle("frozen-seeds", () => observe("node"));
    const rust = canonical(await observe("rust"));
    assert.deepEqual(rust, node);
    // 自检：每个种子都必须真正产出会话与行，避免两侧同时为空而误判一致。
    assert.equal(node.length, seeds.length);
    assert.ok(node.every((entry) => entry.sessions.length > 0));
    assert.ok(node.flatMap((entry) => entry.sessions).some((session) => session.rows.length > 0));
    // membership 种子里的 fork/subagent child/workflow 没有消息历史，只验证可读且行集合为空；
    // archived 会话 Node 不可 resume，按产品保留范围排除在行对拍之外。
    const membership = node.find((entry) => entry.seed === "migration-boundaries.membership");
    assert.deepEqual(membership?.sessions.map((session) => session.id).sort(), [
      "child",
      "fork",
      "legacy",
      "workflow",
    ]);
  },
);
