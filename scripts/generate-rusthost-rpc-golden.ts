import { copyFile, mkdir, mkdtemp, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createFileService } from "../packages/services/src/node.ts";
import { prepareTasksIndexStorage } from "../packages/services/src/storage-startup.ts";
import { TaskIndexRepo } from "../packages/services/src/session/taskIndexRepo.ts";
import { migrateProjectStorage } from "./upstream-sync/project-storage-oracle/project-migrate.ts";

const root = await mkdtemp(join(tmpdir(), "rusthost-rpc-golden-"));
const corpus = join(root, "corpus");
await mkdir(corpus, { recursive: true });
const corpusNames = [
  "_x.txt",
  "a.txt",
  "A.txt",
  "dir_b",
  "dirA",
  "é.txt",
  "README.md",
  "z.txt",
  "Z",
  "1.txt",
  "10.txt",
  "2.txt",
  "a 1",
  "a_1",
  "a-1",
  "a.1",
  "file.TXT",
  "FILE.txt",
  "file(1).txt",
  ".hidden",
];
for (const name of corpusNames) await writeFile(join(corpus, name), "x");
await mkdir(join(corpus, "Zenith"), { recursive: true });
await mkdir(join(corpus, "áz"), { recursive: true });
await symlink(join(corpus, "dirA"), join(corpus, "link-to-dir"));
await symlink(join(corpus, "missing-target"), join(corpus, "dangling-link"));

const fs = createFileService();
const normalize = (entries: unknown) =>
  JSON.parse(JSON.stringify(entries).replaceAll(corpus, "$CORPUS"));
const readdir = {
  visible: normalize(await fs.readdir({ path: corpus })),
  hidden: normalize(await fs.readdir({ path: corpus, includeHidden: true })),
};

const dataBaseDir = join(root, "data-base");
const sqlitePath = join(dataBaseDir, ".escode", "v2", "tasks-index.sqlite");
await prepareTasksIndexStorage(sqlitePath, () => {});
// Rust Host 同时校验 tasks 与 project 两个迁移账本；golden 库也必须走真实 project 迁移。
{
  const projectDb = new (await import("node:sqlite")).DatabaseSync(sqlitePath);
  await migrateProjectStorage(projectDb, {
    backupDirectories: [join(dataBaseDir, ".escode", "v2", "backup")],
  });
  projectDb.close();
}
const repo = new TaskIndexRepo(sqlitePath);
await repo.ensureReady();
const seeded = [
  {
    taskId: "task_min",
    traceId: "escode-task_min",
    title: "最小任务",
    workspacePath: "/w",
    createdAt: 100,
    updatedAt: 200,
    mode: "build" as const,
  },
  {
    taskId: "task_full",
    traceId: "escode-task_full",
    title: "完整任务",
    workspacePath: "/w",
    workspaceIdentity: "/w",
    createdAt: 300,
    updatedAt: 400,
    mode: "plan" as const,
    model: "glm-4",
    provider: "glm" as const,
    cronAutomationId: "cron_1",
    unreadAt: 450,
    status: "running" as const,
  },
];
for (const meta of seeded) await repo.seedTaskMetaIfMissing(meta as never);
const metas: Record<string, unknown> = {};
for (const meta of seeded) {
  metas[meta.taskId] = await repo.getTaskMeta({ workspacePath: "/w", taskId: meta.taskId });
}
const glm = "glm" as never;
const listFixtures = [
  { taskId: "task_l1", provider: glm, title: "列表一", pinned: false, archived: false },
  { taskId: "task_l2", provider: glm, title: "列表二", pinned: true, archived: false },
  { taskId: "task_l3", provider: glm, title: "归档", pinned: false, archived: true },
  { taskId: "task_l4", provider: glm, title: "置顶归档", pinned: true, archived: true },
];
for (const fixture of listFixtures) {
  await repo.seedTaskMetaIfMissing({
    taskId: fixture.taskId,
    traceId: `escode-${fixture.taskId}`,
    title: fixture.title,
    workspacePath: "/w",
    createdAt: 10,
    updatedAt: 20,
    mode: "build",
    provider: glm,
  } as never);
  await repo.syncTaskMeta({
    meta: {
      taskId: fixture.taskId,
      traceId: `escode-${fixture.taskId}`,
      title: fixture.title,
      workspacePath: "/w",
      createdAt: 10,
      updatedAt: 20,
      mode: "build",
      provider: glm,
    },
    pinned: fixture.pinned,
    archived: fixture.archived,
  } as never);
}
const raw = new (await import("node:sqlite")).DatabaseSync(sqlitePath);
for (const [taskId, provider, pinned, archived, updatedAt, searchable] of [
  ["task_q1", "glm", 0, 0, 500, "部署脚本与回滚说明"],
  ["task_q2", "glm", 0, 0, 400, "评审 Rust Host 的迁移"],
  ["task_q3", "glm", 1, 0, 300, "置顶：迁移检查清单"],
  ["task_q4", "glm", 0, 1, 200, "已归档内容"],
  ["task_q5", "claude", 0, 0, 600, "历史导入不应出现"],
  [
    "task_q6",
    "glm",
    0,
    0,
    700,
    "✅ 部署完成后核对迁移清单：迁移包含回滚与灰度，迁移窗口 30 分钟 🙂 结束",
  ],
] as const) {
  raw
    .prepare(
      `INSERT INTO tasks (workspace_key, workspace_path, task_id, title, mode, provider,
       created_at, updated_at, pinned, archived, deleted, searchable_text, meta_json)
     VALUES ('/w', '/w', ?, ?, 'build', ?, 10, ?, ?, ?, 0, ?, ?)`,
    )
    .run(
      taskId,
      `标题 ${taskId}`,
      provider,
      updatedAt,
      pinned,
      archived,
      searchable,
      JSON.stringify({
        taskId,
        traceId: `escode-${taskId}`,
        title: `标题 ${taskId}`,
        workspacePath: "/w",
        createdAt: 10,
        updatedAt,
        mode: "build",
      }),
    );
}
raw
  .prepare(
    `INSERT INTO task_groups (group_id, title, color, created_at, updated_at)
   VALUES ('group_diff', '差分组', 'blue', 10, 20)`,
  )
  .run();
raw
  .prepare(
    `INSERT INTO task_group_members
     (group_id, workspace_key, workspace_path, workspace_identity, task_id, sort_order,
      added_at, created_at, updated_at)
   VALUES ('group_diff', '/w', '/w', NULL, 'task_q1', 7, 30, 30, 30)`,
  )
  .run();
raw
  .prepare(
    `INSERT INTO task_group_view_node_orders (node_type, node_key, sort_order, created_at, updated_at)
   VALUES ('group', 'group_diff', 1, 1, 1), ('task', '["/w","task_q2"]', 2, 1, 1)`,
  )
  .run();
raw.close();

const tsList = (extra: Record<string, unknown>) =>
  repo.listTaskMetas({ workspacePath: "/w", provider: glm, ...extra } as never);
const lists = {
  timeline: await tsList({ pinned: false, archived: false }),
  pinned: await tsList({ pinned: true, archived: false }),
  archived: await tsList({ archived: true }),
  deletedIds: await repo.listDeletedTaskIds({ workspacePath: "/w", provider: glm } as never),
};

const scopes = [{ workspacePath: "/w", workspacePurpose: "project" }];
const queries = [
  { kind: "timeline", sortBy: "updated" },
  { kind: "pinned", sortBy: "updated" },
  { kind: "archived", sortBy: "updated" },
  { kind: "timeline", sortBy: "created", limit: 2 },
  { kind: "timeline", sortBy: "updated", search: "迁移" },
  { kind: "timeline", sortBy: "updated", search: "评审" },
  { kind: "timeline", sortBy: "updated", search: "迁移" },
  { kind: "timeline", sortBy: "updated", search: "🙂" },
] as const;
const taskLists = [];
for (const query of queries) {
  taskLists.push({
    query,
    result: await repo.queryTaskList({
      workspaceScopes: scopes,
      ...query,
      provider: "glm",
    } as never),
  });
}
const grouped = await repo.queryGroupedTaskViewStructure({ workspaceScopes: scopes } as never);

repo.close();
const checkpoint = new (await import("node:sqlite")).DatabaseSync(sqlitePath);
checkpoint.exec("PRAGMA wal_checkpoint(TRUNCATE)");
checkpoint.close();

const golden = {
  _readme:
    "rustHostRpc.test.ts 的 TS oracle 冻结（Phase 3 golden freeze）。由真实 createFileService/TaskIndexRepo 生成一次；删除 TS 后测试只加载本资产。",
  readdir,
  tasks: { metas, lists, taskLists, grouped },
};
const goldenDir = "packages/desktop/test/fixtures/golden";
const goldenJson = `${JSON.stringify(golden, null, 2)}\n`;
const goldenPath = join(goldenDir, "rustHostRpc.oracle.json");
const goldenDb = join(goldenDir, "rustHostRpc.tasks.sqlite");
if (process.argv.includes("--check")) {
  const { deepEqual } = await import("node:assert/strict");
  const committed = JSON.parse(await readFile(goldenPath, "utf8"));
  deepEqual(committed, JSON.parse(goldenJson), "rustHostRpc golden 与当前 TS 观察漂移");
  await readFile(goldenDb).catch(() => {
    throw new Error(`缺少冻结库 ${goldenDb}`);
  });
  console.log("rustHostRpc golden: OK");
} else {
  await mkdir(goldenDir, { recursive: true });
  await writeFile(goldenPath, goldenJson);
  await copyFile(sqlitePath, goldenDb);
  console.log("wrote golden; corpus=", corpus, "db=", sqlitePath);
}
await rm(root, { recursive: true, force: true });
