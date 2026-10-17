// Run with node --import tsx. 用**真实 TS 适配器**导出 `getTaskSnapshot*` / `getTaskConfigOptions`
// 的期望结果，供 Rust 侧 `task_projection` 逐字段对拍（docs/specs/rust-u2-tasks-index.md §6.4.3）。
//
// 提取方式（行为提取，不是文本复制）：`createESCodeTaskServiceAdapter` + 假 `escodeAgentService`
// （`resumeSession`/`readSession` 回放固定 agent 协议快照）+ 真实 `TaskIndexRepo`（临时库，走真实
// `prepareTasksIndexStorage`）。因此**索引行也由 TS 真实写路径产出**，Rust 侧必须复现同一合并语义。
//
// 归一化：`generateTraceId` 是随机 UUID，只出现在权限/追问事件的 `traceId` 上；Rust 侧同样生成随机
// UUID，差分时按占位符 `<traceId>` 归一。ETag 是 `sha256(JSON.stringify(snapshot))`，对键序敏感，
// Rust 的 JSON 键序不同 → 值必然不同（已登记差异），故只断言形态与条件请求语义。
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  TASK_ID,
  WORKSPACE,
  fixtureSnapshot,
} from "./fixtures/escode-host-task-projection-fixture.mjs";
import { createESCodeTaskServiceAdapter } from "../packages/services/src/escode-agent/escodeTaskServiceAdapter.ts";
import { TaskIndexRepo } from "../packages/services/src/session/taskIndexRepo.ts";
import { prepareTasksIndexStorage } from "../packages/services/src/storage-startup.ts";

/** 产物形状变化时递增；Rust 侧加载期断言。 */
const FIXTURE_VERSION = 1;
export /** `generateTraceId` 是随机 UUID：差分前按占位符归一（Rust 侧同样随机生成）。 */
function normalizeRandomTraceIds(snapshot) {
  if (!snapshot || typeof snapshot !== "object") return snapshot;
  const clone = JSON.parse(JSON.stringify(snapshot));
  for (const key of ["pendingPermissions", "pendingElicitations"]) {
    const events = clone.runtime?.[key];
    if (!Array.isArray(events)) continue;
    for (const event of events) {
      if (event && typeof event === "object" && "traceId" in event) event.traceId = "<traceId>";
    }
  }
  return clone;
}

const OUT = "apps/escode-host/crates/host-services/src/task_projection/task_snapshot.golden.json";

async function runVariant(root, variant, snapshot, params, seed) {
  const dir = join(root, variant);
  await mkdir(dir, { recursive: true });
  const dbPath = join(dir, "tasks-index.sqlite");
  await prepareTasksIndexStorage(dbPath, () => {});
  const repo = new TaskIndexRepo(dbPath);
  await repo.ensureReady();
  if (seed) {
    await repo.seedTaskMetaIfMissing(seed);
  }
  const calls = [];
  const agentService = {
    async resumeSession(input) {
      calls.push({ method: "session/resume", params: JSON.parse(JSON.stringify(input ?? null)) });
      return snapshot;
    },
    async readSession(input) {
      calls.push({ method: "session/read", params: JSON.parse(JSON.stringify(input ?? null)) });
      return snapshot;
    },
    async createSession() {
      throw new Error("createSession 不应在快照读面被调用");
    },
  };
  // 读取面不需要真实 syncer（写索引走 repo）；任何被适配器碰到的成员都返回 no-op/可 dispose 形状。
  const disposable = () => ({ dispose() {}, fire() {}, event: () => ({ dispose() {} }) });
  const syncer = new Proxy(
    {},
    {
      get: (_target, key) =>
        key === "getWorkspaceEmitter"
          ? () => ({ fire() {}, event: () => ({ dispose() {} }) })
          : disposable,
    },
  );
  const service = createESCodeTaskServiceAdapter({
    escodeAgentService: agentService,
    taskIndexRepo: repo,
    taskIndexSyncer: syncer,
  });
  try {
    const full = await service.getTaskSnapshot(params);
    const withEtag = await service.getTaskSnapshotWithEtag(params);
    const conditional = full
      ? await service.getTaskSnapshotWithEtag({ ...params, ifNoneMatch: withEtag.etag })
      : null;
    const configOptions = await service.getTaskConfigOptions({ taskId: params.taskId });
    return {
      snapshot: normalizeRandomTraceIds(full),
      // etag 值本身对键序与随机 traceId 都敏感（同一 Host 内自洽），资产只钉形态与条件请求语义。
      etagShape: { length: withEtag.etag.length, hex: /^[0-9a-f]+$/.test(withEtag.etag) },
      // TS 的快照里带 `generateTraceId` 随机值：同参数再请求 etag 必然不同 → 条件请求不命中。
      conditionalRequestHits: conditional?.notModified === true,
      configOptions,
      calls,
    };
  } finally {
    repo.close();
  }
}

async function main() {
  const root = await mkdtemp(join(tmpdir(), "escode-task-projection-"));
  process.env.ESCODE_LOG_DIR = root;
  try {
    const snapshot = fixtureSnapshot();
    const baseParams = { taskId: TASK_ID, workspacePath: WORKSPACE, workspaceIdentity: WORKSPACE };
    const variants = {
      // 桌面 continuous 主链路：无 messageLimit。
      desktop: { params: { ...baseParams, clientMode: "desktop-continuous" } },
      // 尾部窗口：`messageLimit` 生效（history.truncatedBefore）。
      limited: { params: { ...baseParams, clientMode: "desktop-continuous", messageLimit: 2 } },
      // 手机 replayable 首屏：先读索引 hint 再 resume，空 pendingElicitations 也下发。
      replayable: {
        params: { ...baseParams, clientMode: "web-remote-replayable", messageLimit: 4 },
        seed: {
          taskId: TASK_ID,
          traceId: "trace-proj-1",
          title: "索引里的旧标题",
          workspacePath: WORKSPACE,
          workspaceIdentity: WORKSPACE,
          createdAt: 900,
          updatedAt: 950,
          mode: "build",
          model: "glm/glm-4",
          thoughtLevel: "none",
          provider: "glm",
        },
      },
    };
    const expected = {};
    for (const [name, variant] of Object.entries(variants)) {
      expected[name] = {
        params: variant.params,
        // 种子行与输入快照都要落资产：Rust 侧要用**同一批输入**才能逐字段对拍。
        ...(variant.seed ? { seed: variant.seed } : {}),
        result: await runVariant(root, name, snapshot, variant.params, variant.seed),
      };
    }
    // lastError 分支：同一快照带运行时错误，meta.status 归 error。
    const erroneous = fixtureSnapshot();
    erroneous.projection.lastError = {
      type: "runtime",
      code: "rate_limited",
      message: "被限流",
      detail: "429",
      // `attribution` 是对象（`errorAttributionSchema`：source/reason/errorPhase…），
      // 写成字符串会让 meta_json 落成非法行、读回退化成列投影（这正是 TS 的真实行为）。
      attribution: { source: "provider", reason: "被限流" },
    };
    expected.errorSnapshot = {
      params: { ...baseParams, clientMode: "desktop-continuous" },
      input: erroneous,
      result: await runVariant(
        root,
        "errorSnapshot",
        erroneous,
        { ...baseParams, clientMode: "desktop-continuous" },
        null,
      ),
    };

    const golden = {
      version: FIXTURE_VERSION,
      generatedBy: "scripts/generate-escode-host-task-projection-fixtures.mjs",
      input: { snapshot, workspace: WORKSPACE, taskId: TASK_ID },
      expected,
    };
    const text = `${JSON.stringify(golden, null, 2)}\n`;
    if (process.argv.includes("--check")) {
      const current = await readFile(OUT, "utf8");
      if (current !== text) {
        console.error(
          `[task-projection] 期望资产已过期：${OUT}\n运行 node --import tsx ${process.argv[1]} 重新生成`,
        );
        process.exit(1);
      }
      console.log(`[task-projection] 期望资产一致：${OUT}`);
      return;
    }
    await writeFile(OUT, text);
    console.log(`[task-projection] 写入 ${OUT}（${text.length} 字节）`);
    for (const [name, entry] of Object.entries(expected)) {
      const messages = entry.result.snapshot?.messages?.length ?? 0;
      console.log(
        `  - ${name}: messages=${messages} etagLength=${entry.result.etagShape.length} calls=${entry.result.calls.length}`,
      );
    }
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}

await main();
