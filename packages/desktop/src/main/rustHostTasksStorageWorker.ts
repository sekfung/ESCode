import { parentPort, workerData } from "node:worker_threads";
import { z } from "zod";
import { prepareTasksIndexStorage } from "@escode/services/storage-startup";

const data = z
  .object({ path: z.string().min(1) })
  .strict()
  .parse(workerData);

try {
  await prepareTasksIndexStorage(data.path, (phase) =>
    parentPort?.postMessage({ type: "progress", phase }),
  );
  parentPort?.postMessage({ type: "done" });
} catch (error) {
  parentPort?.postMessage({
    type: "failed",
    message: error instanceof Error ? error.message : String(error),
  });
} finally {
  parentPort?.close();
}
