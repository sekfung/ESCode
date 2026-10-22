import { useEffect, useRef } from "react";
import type { IDisposable } from "@escode/rpc";
import type { IFileWatcherService } from "@escode/services";
import { logger } from "@/logger.js";

export interface FileWatchRefreshPath {
  path: string;
  recursive: boolean;
}

interface WatcherRegistration {
  subscription: IDisposable;
  unwatch: () => Promise<void>;
}

/**
 * 监听一组路径，变更后防抖触发 onRefresh。Git 与 SVN 的自动刷新共用这条链路
 * （spec：docs/ui/git-auto-refresh.md、docs/svn/architecture.md），各自只负责计算监听路径。
 * watchPaths 必须由调用方按内容稳定化：引用变化会重建 watcher。
 */
export function useFileWatchRefresh({
  fileWatcherService,
  watchPaths,
  debounceMs,
  onRefresh,
  logScope,
  workspacePath,
}: {
  fileWatcherService: IFileWatcherService;
  watchPaths: readonly FileWatchRefreshPath[];
  debounceMs: number;
  onRefresh: () => void;
  logScope: string;
  workspacePath: string;
}) {
  const refreshRef = useRef(onRefresh);
  const debounceTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  refreshRef.current = onRefresh;

  useEffect(() => {
    let cancelled = false;
    const registrations: WatcherRegistration[] = [];

    const scheduleRefresh = (path: string) => {
      if (debounceTimerRef.current) {
        clearTimeout(debounceTimerRef.current);
      }
      debounceTimerRef.current = setTimeout(() => {
        debounceTimerRef.current = null;
        logger.debug(`[${logScope}] 工作区变更，刷新仓库状态`, {
          workspacePath,
          path,
        });
        refreshRef.current();
      }, debounceMs);
    };

    for (const watchPath of watchPaths) {
      void fileWatcherService
        .watch({
          path: watchPath.path,
          recursive: watchPath.recursive,
        })
        .then(({ id }) => {
          if (cancelled) {
            void fileWatcherService.unwatch({ id });
            return;
          }

          const subscription = fileWatcherService.onDynamicChange(id)((event) => {
            scheduleRefresh(event.dirPath);
          });
          registrations.push({
            subscription,
            unwatch: () => fileWatcherService.unwatch({ id }),
          });
        })
        .catch((error) => {
          if (cancelled) {
            return;
          }
          // 实时刷新只是加速 UI 状态同步；监听失败时保留手动刷新和操作后刷新链路。
          logger.warn(`[${logScope}] 监听工作区失败`, {
            workspacePath,
            path: watchPath.path,
            recursive: watchPath.recursive,
            error: error instanceof Error ? error.message : String(error),
          });
        });
    }

    return () => {
      cancelled = true;
      if (debounceTimerRef.current) {
        clearTimeout(debounceTimerRef.current);
        debounceTimerRef.current = null;
      }
      for (const registration of registrations) {
        registration.subscription.dispose();
        void registration.unwatch().catch((error) => {
          logger.warn(`[${logScope}] 停止监听工作区失败`, {
            workspacePath,
            error: error instanceof Error ? error.message : String(error),
          });
        });
      }
    };
  }, [debounceMs, fileWatcherService, logScope, watchPaths, workspacePath]);
}
