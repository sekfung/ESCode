import { Worker } from "node:worker_threads";
import { z } from "zod";
import {
  getTasksIndexDatabasePath,
  markTasksStoragePrepared,
} from "@escode/services/storage-startup";
import { markLocalDatabaseStartupReady } from "./databaseStartupRelay.js";

const workerMessageSchema = z.discriminatedUnion("type", [
  z
    .object({
      type: z.literal("progress"),
      phase: z.enum([
        "checking",
        "waiting_for_lock",
        "migrating",
        "committing",
        "maintaining",
        "ready",
      ]),
    })
    .strict(),
  z.object({ type: z.literal("done") }).strict(),
  z.object({ type: z.literal("failed"), message: z.string() }).strict(),
]);

/**
 * Rust Host 模式的一次性 tasks-index 准备（docs/specs/rust-u2-tasks-index.md §3.1）。
 *
 * 修复原因：Rust Host 默认开启后本地窗口不再启动 Node Host，而 `TasksIndex::open` 按契约只校验
 * 账本、不建库；此前只有 Node Host 的 `createHostDatabaseStartup` 会调用
 * `prepareTasksIndexStorage`，干净 profile 因此得到“no such table: tasks_schema_migration”。
 * 这里让 Main 在拉起 Rust Host 前复用同一条迁移路径；Rust Host 仍是业务写入的唯一所有者。
 */
export function createRustHostTasksIndexPreparation(options: {
  log?: { info: (...args: unknown[]) => void; warn: (...args: unknown[]) => void };
  /** 测试注入；生产固定走 Main 本地迁移 worker。 */
  prepare?: (path: string) => Promise<void>;
} = {}): () => Promise<void> {
  let preparation: Promise<void> | undefined;
  const prepare = options.prepare ?? ((path: string) => prepareTasksIndex(path, options.log));
  return () => {
    preparation ??= (async () => {
      const path = getTasksIndexDatabasePath();
      options.log?.info(`[rust-host] preparing tasks-index ${path}`);
      try {
        await prepare(path);
        markTasksStoragePrepared(path);
        markLocalDatabaseStartupReady();
        options.log?.info(`[rust-host] tasks-index ready ${path}`);
      } catch (error) {
        // 失败不缓存：Host 重启或用户重试时重新走同一条迁移，不用空库或跳过掩盖锁/损坏。
        preparation = undefined;
        throw error;
      }
    })();
    return preparation;
  };
}

function prepareTasksIndex(
  path: string,
  log?: { info: (...args: unknown[]) => void; warn: (...args: unknown[]) => void },
): Promise<void> {
  return new Promise((resolve, reject) => {
    const worker = new Worker(new URL("./rustHostTasksStorageWorker.js", import.meta.url), {
      workerData: { path },
    });
    let done = false;
    let failure: unknown;
    let firstState = false;
    const firstStateTimer = setTimeout(() => {
      failure = new Error("tasks-index preparation did not report state");
      void worker.terminate();
    }, 30_000);
    worker.on("message", (raw: unknown) => {
      const parsed = workerMessageSchema.safeParse(raw);
      if (!parsed.success) {
        failure = new Error("invalid tasks-index preparation frame");
        void worker.terminate();
        return;
      }
      firstState = true;
      clearTimeout(firstStateTimer);
      const message = parsed.data;
      if (message.type === "progress") {
        if (message.phase === "waiting_for_lock") {
          log?.warn(`[rust-host] tasks-index waiting for lock ${path}`);
        }
      } else if (message.type === "done") {
        done = true;
      } else {
        failure = new Error(message.message);
      }
    });
    worker.once("error", (error) => {
      failure = error;
    });
    worker.once("exit", (code) => {
      clearTimeout(firstStateTimer);
      if (done && code === 0 && !failure && firstState) resolve();
      else reject(failure ?? new Error("tasks-index preparation worker exited before completion"));
    });
  });
}
