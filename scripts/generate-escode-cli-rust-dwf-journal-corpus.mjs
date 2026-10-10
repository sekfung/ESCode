// Run with node --import tsx. `workflows/runs` 的 TS oracle（docs/specs/rust-dynamic-workflow.md 第 4 期前置）：
// 用真实 TS session store 建库（迁移即 DDL），插 dwf_* 行，再跑真实 `listSavedWorkflowRunsOp`。
// 语料 = TS 建出的 DDL + 行 + 各查询的期望结果；Rust 用同一份 DDL/行重建库后自行读，逐字比对。
// --check 防漂移。
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { createSqliteSessionStore } from "../apps/escode-cli/packages/adapters/src/storage/session-store/sqlite-session-store.ts";
import { listSavedWorkflowRunsOp } from "../apps/escode-cli/packages/bootstrap/src/escode-protocol/saved-workflows.ts";
import { createRunIntrospectionMethods } from "../apps/escode-cli/packages/bootstrap/src/app/dynamic-workflow-run-introspection.ts";
import { listWorkflowRunsToolEntry } from "../apps/escode-cli/packages/core/src/tool/handlers/list-workflow-runs.ts";
import { createWorkflowObservationDisplay } from "../apps/escode-cli/packages/core/src/tool/executor/workflow-observation-display.ts";

const check = process.argv.includes("--check");
const target = new URL(
  "../apps/escode-cli-rust/crates/state/tests/fixtures/dwf_journal_corpus.json",
  import.meta.url,
);
const CWD = "/work/project-a";
const OTHER = "/work/project-b";
const NOW = 1_700_000_000_000;
/** 语料固定的"本会话"id：`ownedByThisSession` 与 `possiblyInterrupted` 的判据。 */
const OWNER = "sess_owner";

const run = (overrides) => ({
  id: "run-1",
  parent_session_id: "sess_1",
  cwd: CWD,
  name: "review",
  script_text: "return 1;",
  script_hash: "hash-1",
  args_json: '{"pr":"7"}',
  tool_call_id: "call-1",
  resumed_from: null,
  caps_max_concurrency: 4,
  spent_tokens: 120,
  status: "completed",
  result_json: null,
  failure_json: null,
  time_created: NOW,
  time_updated: NOW,
  ...overrides,
});
const runs = [
  run({ id: "run-completed", time_updated: NOW + 10, script_text: "return 1;" }),
  run({
    id: "run-stopped-user",
    status: "cancelled",
    failure_json: '{"stopReason":"user"}',
    name: null,
    // 标签派生：首个非空行（trim 后）。
    script_text: "\n\n  Triage the issue  \nrest of script\n",
    args_json: null,
    tool_call_id: null,
    time_updated: NOW + 20,
  }),
  run({
    id: "run-stopped-superseded",
    status: "cancelled",
    failure_json: '{"stopReason":"superseded","supersededBy":"run-completed"}',
    name: null,
    // 超长首行 + 一个跨 80 字符边界的 emoji：标签必须截到 80 个 UTF-16 单元且不留孤立代理项。
    script_text: `${"a".repeat(79)}😀tail`,
    time_updated: NOW + 30,
  }),
  run({
    id: "run-stopped-envelope-missing-reason",
    status: "cancelled",
    // 信封里没有 stopReason：嗅探失败 → 退化成 user。
    failure_json: '{"supersededBy":"run-completed"}',
    script_text: "run 4",
    time_updated: NOW + 40,
  }),
  run({
    id: "run-errored",
    status: "failed",
    failure_json: '{"code":"ModelFailure","message":"boom"}',
    // 全空白的 name 与没起名等价；脚本也没有非空行 → 标签回落 runId，来源仍是 script。
    name: "   ",
    script_text: "\n\n",
    time_updated: NOW + 50,
  }),
  run({
    id: "run-interrupted",
    status: "failed",
    failure_json: '{"code":"Interrupted","message":"stopped"}',
    resumed_from: "run-completed",
    time_updated: NOW + 60,
  }),
  // 本会话自己的在飞 run：owned 为真，因此**不**带 possiblyInterrupted。
  run({
    id: "run-running",
    status: "running",
    failure_json: null,
    parent_session_id: OWNER,
    time_updated: NOW + 70,
  }),
  // 兄弟会话在飞的 run：journal 说它没结束，而本会话无法证实 → possiblyInterrupted。
  run({
    id: "run-pending-sibling",
    status: "pending",
    failure_json: null,
    parent_session_id: "sess_sibling",
    name: null,
    script_text: "return 3;",
    time_updated: NOW + 75,
  }),
  run({
    id: "run-pending-other-project",
    status: "pending",
    cwd: OTHER,
    name: "triage",
    script_text: "return 2;",
    parent_session_id: null,
    args_json: "[]",
    spent_tokens: 0,
    time_updated: NOW + 80,
  }),
];

const node = (overrides) => ({
  run_id: "run-completed",
  site_id: "site-1",
  ordinal: 1,
  kind: "artifact",
  actor_site_id: null,
  actor_ordinal: null,
  actor_seq: null,
  input_hash: "hash",
  input_json: null,
  status: "completed",
  result_json: null,
  error_json: null,
  stats_json: null,
  message_boundary: null,
  artifact_id: "report",
  time_created: NOW,
  time_updated: NOW,
  ...overrides,
});
const nodes = [
  // 同一个 id 的两个版本（顶层字段取最新版）+ 一次失败的发布（不进版本历史）。
  node({
    site_id: "artifact-1",
    result_json:
      '{"id":"report","kind":"file","version":1,"title":"v1","contentType":"text/markdown","bytes":10,"publishedAt":1,"uri":"file:///v1"}',
  }),
  node({
    site_id: "artifact-2",
    result_json:
      '{"id":"report","kind":"file","version":2,"title":"v2","description":"desc","contentType":"text/markdown","sourcePath":"/out/v2.md","bytes":20,"publishedAt":2}',
  }),
  node({
    site_id: "artifact-3",
    status: "failed",
    result_json: '{"id":"report","kind":"file","version":3,"title":"never"}',
    error_json: '{"code":"PublishFailed"}',
  }),
  // 预置看板 + 打了同一 id 标签的 report 行（itemCount）。
  node({
    site_id: "artifact-4",
    artifact_id: "board",
    result_json:
      '{"id":"board","kind":"board","version":1,"title":"Board","publishedAt":3,"primary":true}',
  }),
  node({
    site_id: "report-1",
    ordinal: 2,
    kind: "report",
    artifact_id: "board",
    result_json: '{"item":1}',
  }),
  node({
    site_id: "report-2",
    ordinal: 3,
    kind: "report",
    artifact_id: "board",
    result_json: '{"item":2}',
  }),
  // 未打标签的 report 行：不计入 itemCount；也没有版本号（整行丢弃）。
  node({ site_id: "report-3", ordinal: 4, kind: "report", artifact_id: null, result_json: null }),
  // 别的 run 的产物行不该混进来。
  node({ run_id: "run-errored", site_id: "artifact-9", result_json: '{"id":"x","kind":"file","version":1,"publishedAt":0}' }),
];

const root = await mkdtemp(join(tmpdir(), "escode-dwf-journal-"));
try {
  const dbPath = join(root, "ts.sqlite");
  const store = createSqliteSessionStore({ dbPath });
  const db = new DatabaseSync(dbPath);
  const insert = (table, row) => {
    const columns = Object.keys(row);
    db.prepare(
      `insert into ${table} (${columns.join(", ")}) values (${columns.map(() => "?").join(", ")})`,
    ).run(...columns.map((column) => row[column]));
  };
  for (const row of runs) insert("dwf_run", row);
  for (const row of nodes) insert("dwf_node", row);
  // DDL/行原样回读：Rust 侧重建的是**同一份** schema 与数据。
  const tables = ["dwf_run", "dwf_node"];
  // 四张表与全部索引整份带走（rowid 即建表顺序，外键引用先于被引用表）。
  const ddl = db
    .prepare("select sql from sqlite_master where sql is not null and name like 'dwf_%' order by rowid")
    .all()
    .map((row) => row.sql);
  const rows = Object.fromEntries(
    tables.map((table) => [
      table,
      db.prepare(`select * from ${table} order by rowid`).all(),
    ]),
  );

  const context = { deps: { sessionStore: store } };
  const ask = (params) =>
    listSavedWorkflowRunsOp(context, {
      workspace: { workspacePath: CWD, workspaceKey: CWD },
      limit: 50,
      ...params,
    });
  const cases = [
    { label: "project", params: {}, result: await ask({}) },
    { label: "project-limit", params: { limit: 2 }, result: await ask({ limit: 2 }) },
    { label: "project-limit-1", params: { limit: 1 }, result: await ask({ limit: 1 }) },
    { label: "global", params: { scope: "global" }, result: await ask({ scope: "global" }) },
    {
      label: "global-name",
      params: { scope: "global", name: "triage" },
      result: await ask({ scope: "global", name: "triage" }),
    },
    { label: "name", params: { name: "review" }, result: await ask({ name: "review" }) },
    { label: "name-missing", params: { name: "nope" }, result: await ask({ name: "nope" }) },
  ].map(({ label, params, result }) => ({ label, params, result }));

  // `ListWorkflowRuns`（第 6 期）：同一份 journal、注册表为空的读面——端口（内省实现）→ 工具
  // handler → 模型面与 display，全部走真实 TS 代码。
  const methods = createRunIntrospectionMethods({
    introspection: store.workflowJournalStore(),
    journal: store.workflowJournalStore(),
    parentSessionId: OWNER,
    runs: new Map(),
    escalations: new Map(),
    concurrencyCeiling: () => 4,
  });
  const listCases = [];
  for (const params of [{}, { limit: 3 }, { limit: 1 }]) {
    const input = { limit: 20, ...params };
    const result = await listWorkflowRunsToolEntry.handler(input, {
      dynamicWorkflowRunPort: methods,
      workingDirectory: CWD,
    });
    listCases.push({
      label: `list-${params.limit ?? "default"}`,
      input,
      // 端口入参 = 输入 schema 钳制后的 limit（缺省 20），截断探测行由端口自己多取。
      portInput: { cwd: CWD, limit: (input.limit ?? 20) + 1 },
      result,
      modelContent: listWorkflowRunsToolEntry.formatModelContent(result),
      display: createWorkflowObservationDisplay("ListWorkflowRuns", result) ?? null,
    });
  }
  db.close();
  store.close?.();

  const content = `${JSON.stringify({ cwd: CWD, owner: OWNER, ddl, rows, cases, listCases })}\n`;
  if (check) {
    if ((await readFile(target, "utf8").catch(() => "")) !== content)
      throw new Error("Rust dwf journal corpus differs from TS");
  } else await writeFile(target, content);
} finally {
  await rm(root, { recursive: true, force: true });
}
