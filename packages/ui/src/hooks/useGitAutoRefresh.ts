import { useEffect, useMemo, useState } from "react";
import type { GitRepositorySummary } from "@escode/shared";
import {
  buildGitAutoRefreshWatchPaths,
  parseGitAutoRefreshWatchPaths,
  shouldEnableGitAutoRefreshForWorkspace,
  stringifyGitAutoRefreshWatchPaths,
} from "@/lib/gitAutoRefresh.js";
import { useWorkspaceServices } from "@/hooks/useWorkspaceServices.js";
import { useFileWatchRefresh } from "@/hooks/useFileWatchRefresh.js";

// agent 批量写文件时，150ms watcher 防抖 + 350ms Git 防抖仍会把长批次拆成多轮
// `git status`。这里把 Git 自动刷新延后到 1 分钟，降低大工作区里的重复 Git I/O。
const GIT_AUTO_REFRESH_DEBOUNCE_MS = 60_000;

export function useGitAutoRefresh({
  workspacePath,
  workspaceIdentity,
  remoteSessionId,
  gitSummary,
  gitSummaryWorkspaceKey,
  enabled,
  onRefreshGit,
}: {
  workspacePath: string;
  workspaceIdentity?: string | null;
  remoteSessionId?: string | null;
  gitSummary: GitRepositorySummary;
  gitSummaryWorkspaceKey: string;
  enabled: boolean;
  onRefreshGit: () => void;
}) {
  const workspaceServices = useWorkspaceServices(workspacePath, remoteSessionId, workspaceIdentity);
  const { fileWatcherService, systemService } = workspaceServices;
  const currentWorkspaceKey = workspaceIdentity?.trim() || workspacePath;
  const canWatchCurrentWorkspace = shouldEnableGitAutoRefreshForWorkspace({
    enabled,
    currentWorkspaceKey,
    summaryWorkspaceKey: gitSummaryWorkspaceKey,
  });
  const [workspacePlatformState, setWorkspacePlatformState] = useState<{
    service: typeof systemService;
    platform: string;
  } | null>(null);
  // workspaceScopedServices 在远程 workspace 下指向远端 Host，因此这里获取的是
  // WSL/SSH/Docker 的真实运行平台，而不是桌面应用本身的平台。service identity
  // 参与状态匹配，避免 Windows workspace 的旧 platform 泄漏到刚切换的 Linux workspace。
  const workspacePlatform =
    workspacePlatformState?.service === systemService ? workspacePlatformState.platform : null;
  useEffect(() => {
    if (!canWatchCurrentWorkspace || !gitSummary.isGitAvailable || !gitSummary.isRepository) {
      return;
    }

    let cancelled = false;
    void systemService
      .info()
      .then((info) => {
        if (!cancelled) {
          setWorkspacePlatformState({ service: systemService, platform: info.platform });
        }
      })
      .catch(() => {
        // 平台信息不可用时由路径构造器采用 metadata-only 保守策略；手动刷新链路不受影响。
      });

    return () => {
      cancelled = true;
    };
  }, [canWatchCurrentWorkspace, gitSummary.isGitAvailable, gitSummary.isRepository, systemService]);
  const watchPathSignature = useMemo(
    () =>
      stringifyGitAutoRefreshWatchPaths(
        canWatchCurrentWorkspace
          ? buildGitAutoRefreshWatchPaths(
              gitSummary,
              workspacePlatform ? { platform: workspacePlatform } : null,
            )
          : [],
      ),
    [
      canWatchCurrentWorkspace,
      gitSummary.isGitAvailable,
      gitSummary.isRepository,
      gitSummary.repoRoot,
      gitSummary.workspacePath,
      gitSummary.autoRefreshWatchPaths,
      workspacePlatform,
    ],
  );
  const watchPaths = useMemo(
    () => parseGitAutoRefreshWatchPaths(watchPathSignature),
    [watchPathSignature],
  );

  useFileWatchRefresh({
    fileWatcherService,
    watchPaths,
    debounceMs: GIT_AUTO_REFRESH_DEBOUNCE_MS,
    onRefresh: onRefreshGit,
    logScope: "GitAutoRefresh",
    workspacePath,
  });
}
