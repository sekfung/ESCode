// Phase 3 golden freeze：rustHostAutomationParity.test.ts 的 TS `AutomationService` 观察录制。
// 一次性脚本；删除 TS 实现前用于重录。`--check` 校验仓内 golden 与当前 TS 观察一致。
//
// 关键时序：seed 库必须在录制脚本之前快照。后续步骤会创建 / 暂停 / 改名 / 删除 automation，
// 混入 seed 会让 Rust 侧启动时看到额外的行。
//
// 归一化：TS 创建行的动态 id / 时间戳在写库后改写为固定值，使 golden 与 seed 可重复；
// 录制过程中动态生成的行（原用例的 Rust 写者）把 automation id 替换为 RUST_CREATED，
// 并剔除时间戳。其它字段保持 TS 服务的原样投影。
import { deepEqual } from "node:assert/strict";
import { copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { AutomationService } from "../packages/services/src/session/automationService.ts";
import { AutomationRepo } from "../packages/services/src/session/automationRepo.ts";
import { prepareTasksIndexStorage } from "../packages/services/src/storage-startup.ts";
import { migrateProjectStorage } from "./upstream-sync/project-storage-oracle/project-migrate.ts";

const WORKSPACE_PATH = "/tmp/escode-automation-parity-oracle";
const scope = { workspacePath: WORKSPACE_PATH, workspaceIdentity: WORKSPACE_PATH };

// TS 创建行在 seed 中必须稳定：原用例不校验具体 id / 时间戳，这里改成固定值。
const TS_AUTOMATION_ID = "automation-oracle-daily-check";
const TS_CREATED_AT = 1_000;
const TS_UPDATED_AT = 2_000;
const TS_NEXT_RUN_AT = 4_102_444_800_000; // 2100-01-01：避免 Host 启动时把行当作错过的调度窗口改写。
const RUST_LABEL = "RUST_CREATED";

const tsParams = {
  ...scope,
  title: "daily check",
  cronExpr: "0 9 * * *",
  prompt: "check logs",
  recurring: true,
};
const rustParams = {
  ...scope,
  title: "weekly report",
  cronExpr: "0 10 * * 1",
  prompt: "write report",
  recurring: true,
};
const renamedTitle = "daily check (renamed)";

/** 剔除时间戳；把本次录制中动态生成的 automation id 换成稳定标签。 */
function normalize(value: unknown, labels: Map<string, string>): unknown {
  let text = JSON.stringify(value, (key, item) =>
    /^(createdAt|updatedAt|addedAt|anchorAt|nextRunAt|lastRunAt)$/.test(key) ? undefined : item,
  );
  if (text === undefined) return "<undefined>";
  for (const [id, label] of labels) text = text.split(id).join(label);
  return JSON.parse(text);
}

const base = await mkdtemp(join(tmpdir(), "rusthost-automation-parity-golden-"));
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
const repo = new AutomationRepo(db);
const service = new AutomationService(repo);

// 1. TS 创建行：原用例的「TS 写 → Rust 读」起点。
const created = await service.create(tsParams as never);
repo.close();
// 写库后固定动态值，保证 golden JSON 与 seed 库跨次生成一致。
const raw = new (await import("node:sqlite")).DatabaseSync(db);
raw
  .prepare(
    `UPDATE automations
     SET automation_id = ?, created_at = ?, updated_at = ?, next_run_at = ?
     WHERE automation_id = ?`,
  )
  .run(TS_AUTOMATION_ID, TS_CREATED_AT, TS_UPDATED_AT, TS_NEXT_RUN_AT, created.automationId);
raw.exec("PRAGMA wal_checkpoint(TRUNCATE)");
raw.close();

const tsCreated = await service.get(TS_AUTOMATION_ID, scope);
if (!tsCreated) throw new Error("TS 创建的 automation 未落库");
const afterTsCreate = await service.list(scope);

// seed 库必须在后续 mutation 之前快照。
const seedDb = join(base, "seed.sqlite");
await copyFile(db, seedDb);

// 2. 原用例由 Rust 创建第二行、TS 读回。Phase 3 用 TS 服务生成同一投影作为冻结 oracle。
//    create_at 必须晚于 seed 行，列表按 created_at DESC 的 [新行, seed 行] 顺序才稳定。
await new Promise((done) => setTimeout(done, 5));
const rustCreated = await service.create(rustParams as never);
const labels = new Map([[rustCreated.automationId, RUST_LABEL]]);
const expectedRustCreated = normalize(rustCreated, labels);
const afterRustCreate = normalize(await service.list(scope), labels);

// 3. 原用例由 Rust 暂停、TS 读回。
await service.setEnabled(rustCreated.automationId, false, scope);
const paused = await service.get(rustCreated.automationId, scope);
if (!paused) throw new Error("暂停后的 automation 未落库");
const pausedFields = {
  enabled: paused.enabled,
  lifecycleStatus: paused.lifecycleStatus,
};
const pausedRow = normalize(paused, labels);
const afterPause = normalize(await service.list(scope), labels);

// 4. 原用例由 TS 改名、Rust 读回；现在改名也走 Rust updateAutomation，冻结 TS 投影用于比较。
await service.update(TS_AUTOMATION_ID, { title: renamedTitle }, scope);
const afterRename = normalize(await service.list(scope), labels);

// 5. 原用例由 Rust 删除、TS 读回。
await service.delete(rustCreated.automationId, scope);
const afterDelete = normalize(await service.list(scope), labels);
repo.close();

const goldenDir = "packages/desktop/test/fixtures/golden";
const jsonPath = join(goldenDir, "rustHostAutomationParity.oracle.json");
const dbPath = join(goldenDir, "rustHostAutomationParity.seed.sqlite");
const json = `${JSON.stringify(
  {
    _readme:
      "rustHostAutomationParity.test.ts 的 TS AutomationService 观察冻结（Phase 3 golden freeze）。TS 创建行 id/时间戳已固定；录制中动态生成的行 id 归一化为 RUST_CREATED，时间戳已剔除。",
    expected: {
      tsCreated,
      afterTsCreate,
      rustCreated: expectedRustCreated,
      afterRustCreate,
      pausedFields,
      pausedRow,
      afterPause,
      renamedTitle,
      afterRename,
      afterDelete,
    },
  },
  null,
  2,
)}\n`;

if (process.argv.includes("--check")) {
  deepEqual(
    JSON.parse(await readFile(jsonPath, "utf8")),
    JSON.parse(json),
    "rustHostAutomationParity golden 与当前 TS 观察漂移",
  );
  await readFile(dbPath).catch(() => {
    throw new Error(`缺少冻结库 ${dbPath}`);
  });
  console.log("rustHostAutomationParity golden: OK");
} else {
  await mkdir(goldenDir, { recursive: true });
  await writeFile(jsonPath, json);
  await copyFile(seedDb, dbPath);
  console.log("wrote", jsonPath, dbPath);
}
await rm(base, { recursive: true, force: true });
