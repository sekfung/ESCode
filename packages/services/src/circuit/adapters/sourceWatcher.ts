import { watch, type FSWatcher } from "node:fs";
import { stat } from "node:fs/promises";
import { basename, dirname, resolve } from "node:path";
import { createServiceLogger } from "../../logger/serviceLogger.js";
import type { CircuitSourceWatch, CircuitSourceWatcherPort } from "../app/ports.js";

/**
 * 源文件监听适配器（docs/circuit/README.md §3.4）。
 * 监听目标文件的父目录（非递归）并按文件名过滤：这样 KiCad「写临时文件再 rename 覆盖」
 * 保存方式也能拿到回调，文件被删除后再创建也能继续工作（直接监听文件本体在 rename 后就失效）。
 */

const DEFAULT_DEBOUNCE_MS = 150;

export function createFsSourceWatcher(options?: { debounceMs?: number }): CircuitSourceWatcherPort {
  const debounceMs = Math.max(0, options?.debounceMs ?? DEFAULT_DEBOUNCE_MS);
  const log = createServiceLogger("circuit");

  return {
    async watch(params: {
      workspacePath: string;
      relativePath: string;
      onChange: () => void;
    }): Promise<CircuitSourceWatch> {
      const targetPath = resolve(params.workspacePath, params.relativePath);
      const dir = dirname(targetPath);
      const fileName = basename(targetPath);

      // fs.watch 在部分平台对不存在的目录抛异步错误，这里先 stat 统一成同步的 not_found，
      // 由服务决定是降级还是报错。
      let dirInfo;
      try {
        dirInfo = await stat(dir);
      } catch {
        throw new Error(`not_found: 源文件所在目录不存在：${params.relativePath}`);
      }
      if (!dirInfo.isDirectory()) {
        throw new Error(`not_found: 源文件所在目录不存在：${params.relativePath}`);
      }

      let disposed = false;
      let timer: ReturnType<typeof setTimeout> | null = null;
      // Windows 文件名比较不区分大小写。
      const isTargetName = (changedName: string): boolean =>
        process.platform === "win32"
          ? changedName.toLowerCase() === fileName.toLowerCase()
          : changedName === fileName;

      let watcher: FSWatcher;
      try {
        watcher = watch(dir, { persistent: true }, (_eventType, changedName) => {
          if (disposed) return;
          // 拿不到文件名时（少数平台/事件类型）宁可多回调一次：服务会重读并比对内容哈希，
          // 未变化不会误判 stale，漏回调才是真正的失效。
          if (changedName && !isTargetName(changedName.toString())) return;

          if (timer) clearTimeout(timer);
          timer = setTimeout(() => {
            timer = null;
            if (!disposed) params.onChange();
          }, debounceMs);
        });
      } catch (error) {
        throw new Error(
          `io_error: 无法监听源文件目录：${params.relativePath}（${error instanceof Error ? error.message : String(error)}）`,
        );
      }

      watcher.on("error", (error) => {
        // 监听目录被删除或平台 watcher 失效时只告警：抛给调用方就是未捕获异常，可能带崩 Host 进程。
        log.warn(undefined, "电路源文件监听异常", {
          dir,
          fileName,
          error: error instanceof Error ? error.message : String(error),
        });
      });

      return {
        dispose(): void {
          if (disposed) return;
          disposed = true;
          if (timer) {
            clearTimeout(timer);
            timer = null;
          }
          watcher.close();
        },
      };
    },
  };
}
