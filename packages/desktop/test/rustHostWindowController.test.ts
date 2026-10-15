import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { randomUUID } from "node:crypto";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";
// 相对路径 import：根 node_modules 不链接工作区包，测试要拿真实 TS 实现作 oracle。
import { createESCodeTaskServiceAdapter } from "../../services/src/escode-agent/escodeTaskServiceAdapter.js";
import { TaskIndexRepo } from "../../services/src/session/taskIndexRepo.js";
import { prepareTasksIndexStorage } from "../../services/src/storage-startup.js";
import { createWindowHostControllerRuntime } from "../src/host/windowHostControllerService.js";

// docs/specs/rust-u2-window-controller.md §4.2：`listTaskList` / `mutateTask` / 归档删除的
// **真实 TS 服务差分**——同一份 tasks-index 夹具库喂给真实 TS `createWindowHostControllerRuntime`
// （真实 `TaskIndexRepo` + 真实适配器读取面）与 Rust Host 的 `window-controller` channel
// （真实 `@escode/client`），逐字段 `toEqual`。
//
// 链路：spawn escode-host → rpcListen{home, dataBaseDir} → capability → ws://addr/ws/host →
//       connectRpc → services.windowControllerService。

const WORKSPACE_PATH = "/tmp/escode-controller-oracle-workspace";
const WORKSPACE_IDENTITY = "id-controller-oracle";
const WORKSPACE = { workspacePath: WORKSPACE_PATH, workspaceIdentity: WORKSPACE_IDENTITY };

interface TaskSeed {
  taskId: string;
  title: string;
  createdAt: number;
  updatedAt: number;
}

const SEEDS: TaskSeed[] = [
  { taskId: "t-active-new", title: "alpha newest", createdAt: 30, updatedAt: 40 },
  { taskId: "t-active-old", title: "alpha oldest", createdAt: 10, updatedAt: 20 },
  { taskId: "t-pin", title: "beta pinned candidate", createdAt: 15, updatedAt: 25 },
  { taskId: "t-archive", title: "gamma archived candidate", createdAt: 5, updatedAt: 35 },
];

function scope(overrides: { workspacePath: string; workspaceIdentity?: string }) {
  return {
    kind: "local" as const,
    workspacePath: overrides.workspacePath,
    ...(overrides.workspaceIdentity ? { workspaceIdentity: overrides.workspaceIdentity } : {}),
  };
}

/** 读取面适配器的依赖：快照/写面成员不会被本用例触碰，给可 dispose 形状即可。 */
function stubTaskServiceDependencies(repo: TaskIndexRepo) {
  const agentService = new Proxy(
    {},
    {
      get: () => () => {
        throw new Error("provider/agent 路径不应在列表读取面被调用");
      },
    },
  );
  // 与 `generate-escode-host-task-projection-fixtures.mjs` 同法：读取面不驱动 syncer，
  // 任何被碰到的成员都是「调用后回可 dispose 形状」的函数；workspace 事件返回可调用 Event。
  const disposable = () => ({ dispose() {}, fire() {}, event: () => ({ dispose() {} }) });
  // `Event<T>` 是「可调用 + 自带 dispose」的值：workspace 事件注册后永不再触发（本用例不驱动 syncer）。
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

describe("rust host window-controller parity", () => {
  it.skipIf(!existsSync(hostBinary))(
    "listTaskList / mutateTask / archived deletion match the real TS controller runtime",
    async () => {
      const root = await mkdtemp(join(tmpdir(), "escode-rust-host-controller-"));
      const home = join(root, "home");
      const dataBaseDir = join(root, "data-base");
      await mkdir(home, { recursive: true });
      const dbPath = join(dataBaseDir, ".escode", "v2", "tasks-index.sqlite");
      await prepareTasksIndexStorage(dbPath, () => {});
      const repo = new TaskIndexRepo(dbPath);
      await repo.ensureReady();
      for (const seed of SEEDS) {
        await repo.seedTaskMetaIfMissing({
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

      const taskService = stubTaskServiceDependencies(repo);
      const runtime = createWindowHostControllerRuntime({
        createId: () => randomUUID(),
        resolveSource: (queryScope) => ({
          scope: scope(queryScope),
          taskService,
          // 无 agent 服务：与 Rust 侧一致——没有 sessions-index 摘要 → 没有 overlay。
          agentService: undefined,
          sourceAvailability: "online",
        }),
        onSourceError: (scope, operation, error) => {
          console.error("[oracle] source error", operation, scope, error);
        },
      });

      const host = startHost(home);
      try {
        const ready = await nextMeaningful(host);
        expect(ready.kind).toBe("hostReady");
        host.send({ kind: "rpcListen", bind: "127.0.0.1:0", home, dataBaseDir });
        const listening = await nextMeaningful(host);
        expect(listening.kind).toBe("rpcListening");
        const services = await connectRpc(String(listening.addr), String(listening.secret));
        const rust = services.windowControllerService;

        const scopes = [WORKSPACE];
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
        for (const query of queries) {
          const expected = await runtime.service.listTaskList(query as never);
          const actual = await rust.listTaskList(query as never);
          expect(actual, JSON.stringify(query)).toEqual(expected);
        }

        // 写后一致：Rust 侧 pin/archive，两侧再各自读同一库（TS 侧用 `open` mutation 强制重读投影）。
        const address = (taskId: string) => ({ ...WORKSPACE, taskId });
        const pinnedMeta = await rust.mutateTask({
          address: address("t-pin"),
          mutation: { kind: "pin", pinned: true },
        } as never);
        expect((pinnedMeta as { taskId?: string } | null)?.taskId).toBe("t-pin");
        await runtime.service.mutateTask({
          address: address("t-pin"),
          mutation: { kind: "open" },
        } as never);
        expect(
          await rust.listTaskList({
            kind: "pinned",
            sortBy: "updated",
            workspaceScopes: scopes,
          } as never),
        ).toEqual(
          await runtime.service.listTaskList({
            kind: "pinned",
            sortBy: "updated",
            workspaceScopes: scopes,
          } as never),
        );

        await rust.mutateTask({
          address: address("t-archive"),
          mutation: { kind: "archive", archived: true },
        } as never);
        await runtime.service.mutateTask({
          address: address("t-archive"),
          mutation: { kind: "open" },
        } as never);
        for (const kind of ["archived", "active", "timeline"]) {
          expect(
            await rust.listTaskList({ kind, sortBy: "updated", workspaceScopes: scopes } as never),
            kind,
          ).toEqual(
            await runtime.service.listTaskList({
              kind,
              sortBy: "updated",
              workspaceScopes: scopes,
            } as never),
          );
        }

        // 归档删除：空批次短路（两侧都不碰库）。
        const emptyBatch = await rust.deleteArchivedTasks({
          address: address("t-archive"),
          taskIds: [],
        } as never);
        expect(emptyBatch).toEqual({
          deletedTaskIds: [],
          skippedTaskIds: [],
          failedTaskIds: [],
        });
        // 未归档的归档删除 → 幂等跳过（TS 同：`deleteArchivedTask` 回 false）。
        expect(await rust.deleteArchivedTask({ address: address("t-active-new") } as never)).toBe(
          false,
        );
        expect(
          await runtime.service.deleteArchivedTask({ address: address("t-active-new") } as never),
        ).toBe(false);

        // Rust 侧真实删除（Host 是写者 → 事件驱动投影收敛），TS 侧强制重读后逐字段一致。
        expect(
          await rust.deleteArchivedTasks({
            address: address("t-archive"),
            taskIds: ["t-archive"],
          } as never),
        ).toEqual({ deletedTaskIds: ["t-archive"], skippedTaskIds: [], failedTaskIds: [] });
        await runtime.service.mutateTask({
          address: address("t-active-new"),
          mutation: { kind: "open" },
        } as never);
        expect(
          await rust.listTaskList({
            kind: "archived",
            sortBy: "updated",
            workspaceScopes: scopes,
          } as never),
        ).toEqual(
          await runtime.service.listTaskList({
            kind: "archived",
            sortBy: "updated",
            workspaceScopes: scopes,
          } as never),
        );

        // 反向：TS 侧写（外部写者）→ Rust 侧强制重读（`open` mutation）后同样一致。
        await runtime.service.mutateTask({
          address: address("t-active-old"),
          mutation: { kind: "archive", archived: true },
        } as never);
        await rust.mutateTask({
          address: address("t-active-old"),
          mutation: { kind: "open" },
        } as never);
        for (const kind of ["archived", "active", "timeline"]) {
          expect(
            await rust.listTaskList({ kind, sortBy: "updated", workspaceScopes: scopes } as never),
            kind,
          ).toEqual(
            await runtime.service.listTaskList({
              kind,
              sortBy: "updated",
              workspaceScopes: scopes,
            } as never),
          );
        }

        // 地址校验文案（TS 抛错 → Rust 回结构化错误）。
        await expect(
          rust.mutateTask({
            address: { ...WORKSPACE, taskId: "missing-task" },
            mutation: { kind: "pin", pinned: true },
          } as never),
        ).rejects.toThrow("没有与任务地址匹配的 source");
      } finally {
        repo.close();
        await host.stop();
      }
    },
  );
});
