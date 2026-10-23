import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { copyFile, mkdir, mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

// Phase 3 golden freeze：原「真实 TS `createWindowHostControllerRuntime` + `TaskIndexRepo`」oracle
// 的逐步观察已冻结（时间戳剔除），测试不再实例化 TS runtime/Repo；Rust Host 从冻结 seed 库启动。
const goldenFile = new URL(
  "./fixtures/golden/rustHostWindowController.oracle.json",
  import.meta.url,
);
const goldenSeed = new URL(
  "./fixtures/golden/rustHostWindowController.seed.sqlite",
  import.meta.url,
);

// docs/specs/rust-u2-window-controller.md §4.2：`listTaskList` / `mutateTask` / 归档删除的
// Rust Host `window-controller` channel（真实 `@escode/client`）对冻结 TS 观察逐字段比较。
//
// 链路：spawn escode-host → rpcListen{home, dataBaseDir} → capability → ws://addr/ws/host →
//       connectRpc → services.windowControllerService。

const WORKSPACE_PATH = "/tmp/escode-controller-oracle-workspace";
const WORKSPACE_IDENTITY = "id-controller-oracle";
const WORKSPACE = { workspacePath: WORKSPACE_PATH, workspaceIdentity: WORKSPACE_IDENTITY };

interface TaskListResult {
  items: unknown[];
  total: number;
  hasMore: boolean;
}

interface DeletionResult {
  deletedTaskIds: string[];
  skippedTaskIds: string[];
  failedTaskIds: string[];
}

interface Golden {
  expected: {
    initialQueries: TaskListResult[];
    pin: { taskId: string };
    pinnedAfterPin: TaskListResult;
    archive: { taskId: string };
    afterArchive: Record<string, TaskListResult>;
    emptyBatch: DeletionResult;
    deleteActive: boolean;
    deleteArchived: DeletionResult;
    afterDeleteArchived: TaskListResult;
    archiveOld: { taskId: string };
    afterArchiveOld: Record<string, TaskListResult>;
    missingError: string;
  };
}

/** 与生成脚本一致：剔除时间戳；本用例的 task id 都是固定 seed，无需归一化。 */
function normalize(value: unknown): unknown {
  const text = JSON.stringify(value, (key, item) =>
    /^(created|updated|added)At$/.test(key) ? undefined : item,
  );
  return text === undefined ? "<undefined>" : JSON.parse(text);
}

const scopes = [WORKSPACE];
const address = (taskId: string) => ({ ...WORKSPACE, taskId });
const listQuery = (kind: string) => ({ kind, sortBy: "updated", workspaceScopes: scopes });

describe("rust host window-controller parity", () => {
  it.skipIf(!existsSync(hostBinary))(
    "listTaskList / mutateTask / archived deletion match the frozen TS controller observations",
    async () => {
      const root = await mkdtemp(join(tmpdir(), "escode-rust-host-controller-"));
      const home = join(root, "home");
      const dataBaseDir = join(root, "data-base");
      await mkdir(home, { recursive: true });
      // 冻结的 TS tasks-index seed：Rust Host 直接读同一文件，不再在测试内跑 TS Repo 造数。
      const dbDir = join(dataBaseDir, ".escode", "v2");
      await mkdir(dbDir, { recursive: true });
      await copyFile(goldenSeed, join(dbDir, "tasks-index.sqlite"));
      const golden = JSON.parse(await readFile(goldenFile, "utf8")) as Golden;
      const expected = golden.expected;

      const host = startHost(home);
      try {
        const ready = await nextMeaningful(host);
        expect(ready.kind).toBe("hostReady");
        host.send({ kind: "rpcListen", bind: "127.0.0.1:0", home, dataBaseDir });
        const listening = await nextMeaningful(host);
        expect(listening.kind).toBe("rpcListening");
        const services = await connectRpc(String(listening.addr), String(listening.secret));
        const rust = services.windowControllerService;

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
        for (const [index, query] of queries.entries()) {
          const actual = await rust.listTaskList(query as never);
          expect(normalize(actual), JSON.stringify(query)).toEqual(expected.initialQueries[index]);
        }

        // 写后一致：Rust 侧 pin/archive，Host 自己是写者，事件驱动投影收敛后对冻结观察。
        const pinnedMeta = await rust.mutateTask({
          address: address("t-pin"),
          mutation: { kind: "pin", pinned: true },
        } as never);
        expect((pinnedMeta as { taskId?: string } | null)?.taskId).toBe(expected.pin.taskId);
        expect(normalize(await rust.listTaskList(listQuery("pinned") as never))).toEqual(
          expected.pinnedAfterPin,
        );

        await rust.mutateTask({
          address: address("t-archive"),
          mutation: { kind: "archive", archived: true },
        } as never);
        for (const kind of ["archived", "active", "timeline"]) {
          expect(normalize(await rust.listTaskList(listQuery(kind) as never)), kind).toEqual(
            expected.afterArchive[kind],
          );
        }

        // 归档删除：空批次短路（不碰库）。
        expect(
          await rust.deleteArchivedTasks({
            address: address("t-archive"),
            taskIds: [],
          } as never),
        ).toEqual(expected.emptyBatch);
        // 未归档的归档删除 → 幂等跳过。
        expect(await rust.deleteArchivedTask({ address: address("t-active-new") } as never)).toBe(
          expected.deleteActive,
        );

        // Rust 侧真实删除（Host 是写者 → 事件驱动投影收敛）。
        expect(
          await rust.deleteArchivedTasks({
            address: address("t-archive"),
            taskIds: ["t-archive"],
          } as never),
        ).toEqual(expected.deleteArchived);
        expect(normalize(await rust.listTaskList(listQuery("archived") as never))).toEqual(
          expected.afterDeleteArchived,
        );

        // 反向写者（原用例由 TS 侧写入）：归档 t-active-old 后三分区一致。
        await rust.mutateTask({
          address: address("t-active-old"),
          mutation: { kind: "archive", archived: true },
        } as never);
        for (const kind of ["archived", "active", "timeline"]) {
          expect(normalize(await rust.listTaskList(listQuery(kind) as never)), kind).toEqual(
            expected.afterArchiveOld[kind],
          );
        }

        // 地址校验文案：与冻结的 TS 观察一致。
        await expect(
          rust.mutateTask({
            address: { ...WORKSPACE, taskId: "missing-task" },
            mutation: { kind: "pin", pinned: true },
          } as never),
        ).rejects.toThrow(expected.missingError);
      } finally {
        await host.stop();
        await rm(root, { recursive: true, force: true });
      }
    },
  );
});
