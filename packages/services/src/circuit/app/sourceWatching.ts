import { createServiceLogger } from "../../logger/serviceLogger.js";
import type { CircuitLocation } from "./circuitLocation.js";
import type { CircuitSourceWatch, CircuitSourceWatcherPort } from "./ports.js";

const log = createServiceLogger("circuit");

/**
 * 源文件监听装配（§3.4）：对这次解析真正读到的文件（含子图纸）建监听，变化时回调标记 stale。
 * 监听失败（文件不存在、平台不支持）不阻塞解析：IR 仍可用，只是不会自动标 stale；
 * 失败原因记 warn，只记路径与错误消息，不写文件内容（§9）。
 */
export async function watchCircuitSourceFiles(params: {
  watcher: CircuitSourceWatcherPort;
  location: CircuitLocation;
  irId: string;
  sourceFiles: string[];
  onChange: () => void;
}): Promise<CircuitSourceWatch[]> {
  const watches: CircuitSourceWatch[] = [];
  for (const relativePath of params.sourceFiles) {
    try {
      watches.push(
        await params.watcher.watch({
          workspacePath: params.location.workspacePath,
          relativePath,
          onChange: params.onChange,
        }),
      );
    } catch (error) {
      log.warn("circuit source watch failed", {
        irId: params.irId,
        path: relativePath,
        message: error instanceof Error ? error.message : String(error),
      });
    }
  }
  return watches;
}
