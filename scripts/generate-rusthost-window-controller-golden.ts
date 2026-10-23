// Phase 3 golden freeze：rustHostWindowController.test.ts 的 TS controller runtime 观察录制。
// 一次性脚本；删除 TS 实现前用于重录。`--check` 校验仓内 golden 与当前 TS 观察一致。
//
// 关键时序：seed 库必须在录制脚本之前快照。录制会 pin / archive / delete 任务，
// 若先录制再快照，Rust 侧启动时看到的就是已被改写的库。
import { deepEqual } from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createESCodeTaskServiceAdapter } from "../packages/services/src/escode-agent/escodeTaskServiceAdapter.ts";
import { TaskIndexRepo } from "../packages/services/src/session/taskIndexRepo.ts";
import { prepareTasksIndexStorage } from "../packages/services/src/storage-startup.ts";
import { createWindowHostControllerRuntime } from "../packages/desktop/src/host/windowHostControllerService.ts";

const WORKSPACE_PATH = "/tmp/escode-controller-oracle-workspace";
const WORKSPACE_IDENTITY = "id-controller-oracle";
const WORKSPACE = { workspacePath: WORKSPACE_PATH, workspaceIdentity: WORKSPACE_IDENTITY };
const scopes = [WORKSPACE];

const SEEDS = [
  { taskId: "t-active-new", title: "alpha newest", createdAt: 30, updatedAt: 40 },
  { taskId: "t-active-old", title: "alpha oldest", createdAt: 10, updatedAt: 20 },
  { taskId: "t-pin", title: "beta pinned candidate", createdAt: 15, updatedAt: 25 },
  { taskId: "t-archive", title: "gamma archived candidate", createdAt: 5, updatedAt: 35 },
];

const queries = [
  { kind: "active", sortBy: "updated", workspaceScopes: scopes },
  { kind: "timeline", sortBy: "updated", workspaceScopes: scopes },
  { kind: "pinned", sortBy: "updated", workspaceScopes: scopes },
  { kind: "archived", sortBy: "updated", workspaceScopes: scopes },
  { kind: "active", sortBy: "created", workspaceScopes: scopes },
  { kind: "active", sortBy: "updated", workspaceScopes: scopes, limit: 2 },
  { kind: "active", sortBy: "updated", workspaceScopes: scopes, search: "alpha" },
  { kind: "active", sortBy: "updated", workspaceScopes: scopes, search: "no-match-here" },
];

const listQuery = (kind: string) => ({ kind, sortBy: "updated", workspaceScopes: scopes });
const address = (taskId: string) => ({ ...WORKSPACE, taskId });

/** 与 rustHostTaskGroups 一致：剔除时间戳；本用例的 task id 都是固定 seed，无需归一化。 */
function normalize(value: unknown): unknown {
  const text = JSON.stringify(value, (key, item) =>
    /^(created|updated|added)At$/.test(key) ? undefined : item,
  );
  return text === undefined ? "<undefined>" : JSON.parse(text);
}

/** 读取面适配器的依赖：与测试相同的可 dispose 形状，列表读取不会触碰 provider/agent。 */
function stubTaskServiceDependencies(repo: TaskIndexRepo) {
  const agentService = new Proxy(
    {},
    {
      get: () => () => {
        throw new Error("provider/agent 路径不应在列表读取面被调用");
      },
    },
  );
  const disposable = () => ({ dispose() {}, fire() {}, event: () => ({ dispose() {} }) });
  const workspaceEvent = () => {
    const event = () => ({ dispose: () => undefined });
    return Object.assign(event, { dispose: () => undefined });
  };
  const syncer = new Proxy(
    {},
    {
      get: (_target, key) =>
        key === "onDynamicWorkspaceEvent" || key === "getWorkspaceEmitter"
          ? workspaceEvent
          : disposable,
    },
  );
  return createESCodeTaskServiceAdapter({
    escodeAgentService: agentService as never,
    taskIndexRepo: repo,
    taskIndexSyncer: syncer as never,
  });
}

interface Driver {
  list(query: unknown): Promise<unknown>;
  mutate(address: unknown, mutation: unknown): Promise<unknown>;
  deleteArchivedTasks(address: unknown, taskIds: string[]): Promise<unknown>;
  deleteArchivedTask(address: unknown): Promise<unknown>;
}

function runtimeDriver(runtime: ReturnType<typeof createWindowHostControllerRuntime>): Driver {
  return {
    list: (query) => runtime.service.listTaskList(query as never),
    mutate: (taskAddress, mutation) =>
      runtime.service.mutateTask({ address: taskAddress, mutation } as never),
    deleteArchivedTasks: (taskAddress, taskIds) =>
      runtime.service.deleteArchivedTasks({ address: taskAddress, taskIds } as never),
    deleteArchivedTask: (taskAddress) =>
      runtime.service.deleteArchivedTask({ address: taskAddress } as never),
  };
}

async function script(driver: Driver): Promise<Record<string, unknown>> {
  const out: Record<string, unknown> = {};

  out.initialQueries = [];
  for (const query of queries) {
    (out.initialQueries as unknown[]).push(normalize(await driver.list(query)));
  }

  // Rust 侧 pin：写后列表。
  out.pin = normalize(await driver.mutate(address("t-pin"), { kind: "pin", pinned: true }));
  out.pinnedAfterPin = normalize(await driver.list(listQuery("pinned")));

  // Rust 侧 archive：三分区列表。
  out.archive = normalize(
    await driver.mutate(address("t-archive"), { kind: "archive", archived: true }),
  );
  out.afterArchive = {};
  for (const kind of ["archived", "active", "timeline"]) {
    (out.afterArchive as Record<string, unknown>)[kind] = normalize(
      await driver.list(listQuery(kind)),
    );
  }

  // 归档删除：空批次、未归档跳过、真实删除。
  out.emptyBatch = normalize(await driver.deleteArchivedTasks(address("t-archive"), []));
  out.deleteActive = normalize(await driver.deleteArchivedTask(address("t-active-new")));
  out.deleteArchived = normalize(
    await driver.deleteArchivedTasks(address("t-archive"), ["t-archive"]),
  );
  out.afterDeleteArchived = normalize(await driver.list(listQuery("archived")));

  // 反向写者（原用例由 TS 侧写入，Rust 侧再 open 重读）：归档 t-active-old。
  out.archiveOld = normalize(
    await driver.mutate(address("t-active-old"), { kind: "archive", archived: true }),
  );
  out.afterArchiveOld = {};
  for (const kind of ["archived", "active", "timeline"]) {
    (out.afterArchiveOld as Record<string, unknown>)[kind] = normalize(
      await driver.list(listQuery(kind)),
    );
  }

  // 地址校验：未登记 task 必须报同一结构性错误。
  out.missingError = await driver
    .mutate(address("missing-task"), { kind: "pin", pinned: true })
    .then(
      () => "did not throw",
      (error: Error) => error.message,
    );

  return out;
}

const base = await mkdtemp(join(tmpdir(), "rusthost-window-controller-golden-"));
const db = join(base, "tasks-index.sqlite");
await prepareTasksIndexStorage(db, () => {});
const seedRepo = new TaskIndexRepo(db);
await seedRepo.ensureReady();
for (const seed of SEEDS) {
  await seedRepo.seedTaskMetaIfMissing({
    taskId: seed.taskId,
    traceId: `trace-${seed.taskId}`,
    title: seed.title,
    workspacePath: WORKSPACE_PATH,
    workspaceIdentity: WORKSPACE_IDENTITY,
    createdAt: seed.createdAt,
    updatedAt: seed.updatedAt,
    mode: "build",
    provider: "glm",
    model: "fixture/fixture-model$none",
    thoughtLevel: "none",
  } as never);
}
seedRepo.close();
const checkpoint = new (await import("node:sqlite")).DatabaseSync(db);
checkpoint.exec("PRAGMA wal_checkpoint(TRUNCATE)");
checkpoint.close();
// seed 库必须在录制脚本之前快照：录制会 pin / archive / delete，混入后会改写初始 4-task seed。
const seedDb = join(base, "seed.sqlite");
await copyFile(db, seedDb);

const recording = new TaskIndexRepo(db);
await recording.ensureReady();
const taskService = stubTaskServiceDependencies(recording);
const runtime = createWindowHostControllerRuntime({
  createId: () => randomUUID(),
  resolveSource: () => ({
    scope: { kind: "local" as const, ...WORKSPACE },
    taskService,
    // 无 agent 服务：与 Rust 侧一致——没有 sessions-index 摘要 → 没有 overlay。
    agentService: undefined,
    sourceAvailability: "online",
  }),
  onSourceError: (scope, operation, error) => {
    console.error("[golden] source error", operation, scope, error);
  },
});
const golden = await script(runtimeDriver(runtime));
runtime.dispose();
recording.close();

const goldenDir = "packages/desktop/test/fixtures/golden";
const jsonPath = join(goldenDir, "rustHostWindowController.oracle.json");
const dbPath = join(goldenDir, "rustHostWindowController.seed.sqlite");
const json = `${JSON.stringify(
  {
    _readme:
      "rustHostWindowController.test.ts 的 TS controller runtime 观察冻结（Phase 3 golden freeze）。时间戳已剔除；task id 均为固定 seed。",
    expected: golden,
  },
  null,
  2,
)}\n`;
if (process.argv.includes("--check")) {
  deepEqual(
    JSON.parse(await readFile(jsonPath, "utf8")),
    JSON.parse(json),
    "rustHostWindowController golden 与当前 TS 观察漂移",
  );
  await readFile(dbPath).catch(() => {
    throw new Error(`缺少冻结库 ${dbPath}`);
  });
  console.log("rustHostWindowController golden: OK");
} else {
  await mkdir(goldenDir, { recursive: true });
  await writeFile(jsonPath, json);
  await copyFile(seedDb, dbPath);
  console.log("wrote", jsonPath, dbPath);
}
await rm(base, { recursive: true, force: true });
