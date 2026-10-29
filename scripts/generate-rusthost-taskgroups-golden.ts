// Phase 3 golden freeze：rustHostTaskGroups.test.ts 的 TS `TaskIndexRepo` 观察录制。
// 一次性脚本；删除 TS 实现前用于重录。`--check` 校验仓内 golden 与当前 TS 观察一致。
import { deepEqual } from "node:assert/strict";
import { copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { prepareTasksIndexStorage } from "../packages/services/src/storage-startup.ts";
import { migrateProjectStorage } from "./upstream-sync/project-storage-oracle/project-migrate.ts";
import { TaskIndexRepo } from "../packages/services/src/session/taskIndexRepo.ts";

const glm = "glm" as never;
const scopes = [{ workspacePath: "/w" }];
const ref = (taskId: string, workspacePath = "/w") => ({ workspacePath, taskId });

async function seed(repo: TaskIndexRepo): Promise<void> {
  await repo.ensureReady();
  for (const [taskId, extra] of [
    ["t1", {}],
    ["t2", {}],
    ["t3", {}],
    ["t_archived", { archived: true }],
  ] as const) {
    const meta = {
      taskId,
      traceId: `escode-${taskId}`,
      title: taskId,
      workspacePath: "/w",
      createdAt: 10,
      updatedAt: 20,
      mode: "build",
      provider: glm,
    };
    await repo.seedTaskMetaIfMissing(meta as never);
    await repo.syncTaskMeta({ meta, pinned: false, archived: "archived" in extra } as never);
  }
}

function normalize(value: unknown, ids: Map<string, string>): unknown {
  let text = JSON.stringify(value, (key, item) =>
    /^(created|updated|added)At$/.test(key) ? undefined : item,
  );
  if (text === undefined) return "<undefined>";
  for (const [id, label] of ids) text = text.split(id).join(label);
  return JSON.parse(text);
}

interface Driver {
  create(params: { title?: string; color?: string }): Promise<{ id: string }>;
  rename(groupId: string, title: string): Promise<unknown>;
  recolor(groupId: string, color: string): Promise<unknown>;
  remove(groupId: string): Promise<unknown>;
  apply(params: { topLevelNodes: unknown[]; groups: unknown[] }): Promise<unknown>;
  structure(): Promise<unknown>;
}

function repoDriver(repo: TaskIndexRepo): Driver {
  return {
    create: (params) => repo.createTaskGroup(params as never),
    rename: (groupId, title) => repo.renameTaskGroup({ groupId, title }),
    recolor: (groupId, color) => repo.updateTaskGroupColor({ groupId, color: color as never }),
    remove: (groupId) => repo.deleteTaskGroup({ groupId }),
    apply: (params) =>
      repo.applyGroupedTaskViewOrder({
        workspaceScopes: scopes,
        provider: glm,
        ...params,
      } as never),
    structure: () => repo.queryGroupedTaskViewStructure({ workspaceScopes: scopes } as never),
  };
}

async function script(driver: Driver): Promise<Record<string, unknown>> {
  const out: Record<string, unknown> = {};
  const ids = new Map<string, string>();
  const label = (group: { id: string }, name: string) => {
    ids.set(group.id, name);
    return group;
  };
  const settle = async (run: () => Promise<unknown>) =>
    run().then(
      (value) => ({ ok: normalize(value, ids) }),
      (error: Error) => ({ error: error.message }),
    );
  const snapshot = async (name: string) => {
    out[name] = normalize(await driver.structure(), ids);
  };

  const g1 = label(await driver.create({ title: "  Alpha ", color: "red" }), "G1");
  const g2 = label(await driver.create({}), "G2");
  out.created = normalize([g1, g2], ids);
  await snapshot("afterCreate");

  out.renameBlank = await settle(() => driver.rename(g1.id, "   "));
  out.recolor = await settle(() => driver.recolor(g2.id, "purple"));
  out.recolorInvalid = await settle(() => driver.recolor(g2.id, "pink"));
  out.renameMissing = await settle(() => driver.rename("task-group-ghost", "x"));
  out.recolorMissing = await settle(() => driver.recolor("task-group-ghost", "red"));
  await snapshot("afterEdits");

  out.apply = await settle(() =>
    driver.apply({
      topLevelNodes: [
        { type: "task", task: ref("t3") },
        { type: "group", groupId: g2.id },
        { type: "group", groupId: g1.id },
      ],
      groups: [
        { groupId: g1.id, taskRefs: [ref("t2"), ref("t1")] },
        { groupId: g2.id, taskRefs: [] },
      ],
    }),
  );
  await snapshot("afterApply");

  out.applyMove = await settle(() =>
    driver.apply({
      topLevelNodes: [
        { type: "group", groupId: g1.id },
        { type: "task", task: ref("t1") },
        { type: "group", groupId: g2.id },
      ],
      groups: [],
    }),
  );
  await snapshot("afterMove");

  out.applyDuplicate = await settle(() =>
    driver.apply({
      topLevelNodes: [],
      groups: [
        { groupId: g1.id, taskRefs: [ref("t1")] },
        { groupId: g2.id, taskRefs: [ref("t1")] },
      ],
    }),
  );
  out.applyGhostGroup = await settle(() =>
    driver.apply({ topLevelNodes: [{ type: "group", groupId: "task-group-ghost" }], groups: [] }),
  );
  out.applyOutsideScope = await settle(() =>
    driver.apply({ topLevelNodes: [{ type: "task", task: ref("t1", "/other") }], groups: [] }),
  );
  out.applyArchived = await settle(() =>
    driver.apply({ topLevelNodes: [{ type: "task", task: ref("t_archived") }], groups: [] }),
  );
  out.applyMissing = await settle(() =>
    driver.apply({ topLevelNodes: [{ type: "task", task: ref("nope") }], groups: [] }),
  );
  await snapshot("afterRejected");

  out.remove = await settle(() => driver.remove(g2.id));
  out.removeAgain = await settle(() => driver.remove(g2.id));
  await snapshot("afterRemove");
  return out;
}

const base = await mkdtemp(join(tmpdir(), "rusthost-taskgroups-golden-"));
const db = join(base, "tasks-index.sqlite");
await prepareTasksIndexStorage(db, () => {});
// Rust Host 同时校验 tasks 与 project 账本；golden 种子库也要走真实 project 迁移。
{
  const projectDb = new (await import("node:sqlite")).DatabaseSync(db);
  await migrateProjectStorage(projectDb, {
    backupDirectories: [new URL(".", import.meta.url).pathname],
  });
  projectDb.close();
}
const repo = new TaskIndexRepo(db);
await seed(repo);
repo.close();
const checkpoint = new (await import("node:sqlite")).DatabaseSync(db);
checkpoint.exec("PRAGMA wal_checkpoint(TRUNCATE)");
checkpoint.close();
// seed 库必须在录制脚本之前快照：脚本会创建分组，混入 seed 会让 Rust 侧看到额外分组。
const seedDb = join(base, "seed.sqlite");
await copyFile(db, seedDb);
const recording = new TaskIndexRepo(db);
await recording.ensureReady();
const golden = await script(repoDriver(recording));
recording.close();

const goldenDir = "packages/desktop/test/fixtures/golden";
const jsonPath = join(goldenDir, "rustHostTaskGroups.oracle.json");
const dbPath = join(goldenDir, "rustHostTaskGroups.seed.sqlite");
const json = `${JSON.stringify(
  {
    _readme:
      "rustHostTaskGroups.test.ts 的 TS TaskIndexRepo 观察冻结（Phase 3 golden freeze）。group id 已归一化为 G1/G2，时间戳已剔除。",
    expected: golden,
  },
  null,
  2,
)}\n`;
if (process.argv.includes("--check")) {
  deepEqual(
    JSON.parse(await readFile(jsonPath, "utf8")),
    JSON.parse(json),
    "rustHostTaskGroups golden 与当前 TS 观察漂移",
  );
  await readFile(dbPath).catch(() => {
    throw new Error(`缺少冻结库 ${dbPath}`);
  });
  console.log("rustHostTaskGroups golden: OK");
} else {
  await mkdir(goldenDir, { recursive: true });
  await writeFile(jsonPath, json);
  await copyFile(seedDb, dbPath);
  console.log("wrote", jsonPath, dbPath);
}
await rm(base, { recursive: true, force: true });
