import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ISvnService } from "@escode/services";
import type {
  SvnFailure,
  SvnFileChange,
  SvnRemoteStatusResult,
  SvnRequest,
  SvnWorkingCopyInfo,
  WorkspaceVcsInfo,
  WorkspaceVcsOverride,
} from "@escode/shared";
import { useWorkspaceServices } from "@/hooks/useWorkspaceServices.js";
import { useFileWatchRefresh, type FileWatchRefreshPath } from "@/hooks/useFileWatchRefresh.js";
import { useSettings } from "@/hooks/useSettingService.js";
import { joinFilePath } from "@/lib/path.js";
import { logger } from "@/logger.js";
import { subscribeWorkspaceVcsChanged } from "@/svn/workspaceVcsEvents.js";

/** 与 Git 自动刷新保持同一防抖参数（docs/ui/git-auto-refresh.md）。 */
const SVN_AUTO_REFRESH_DEBOUNCE_MS = 60_000;
/** 服务器新修订的低频检查间隔（docs/svn/requirements.md 非功能要求）。 */
export const SVN_REMOTE_CHECK_INTERVAL_MS = 5 * 60_000;

/** 旧 Host 不提供 workspace-vcs channel 时，保持旧行为：一律按 Git 处理。 */
const LEGACY_GIT_VCS_INFO: WorkspaceVcsInfo = { kind: "git", source: "detected", override: "auto" };

export interface SvnWorkspaceState {
  workspaceKey: string;
  info: SvnWorkingCopyInfo | null;
  changes: SvnFileChange[];
  ignoredPaths: string[];
  loading: boolean;
  failure: SvnFailure | null;
  /** 每次成功刷新递增，供 diff 缓存失效。 */
  revision: number;
  remote: SvnRemoteStatusResult | null;
}

export interface SvnWorkspaceController {
  workspacePath: string;
  request: SvnRequest;
  svnService: ISvnService | null;
  state: SvnWorkspaceState;
  refresh: () => void;
  refreshRemote: () => void;
}

export interface WorkspaceVcsController {
  vcs: WorkspaceVcsInfo | null;
  setOverride: (override: WorkspaceVcsOverride) => Promise<void>;
  svn: SvnWorkspaceController | null;
}

function createEmptyState(workspaceKey: string): SvnWorkspaceState {
  return {
    workspaceKey,
    info: null,
    changes: [],
    ignoredPaths: [],
    loading: false,
    failure: null,
    revision: 0,
    remote: null,
  };
}

function toUnsupportedHostInfo(): SvnWorkingCopyInfo {
  return { availability: { status: "unsupported-host" }, isWorkingCopy: false };
}

/**
 * 计算 SVN 自动刷新监听路径：非 Linux 递归监听 workspace；Linux 递归监听代价高，
 * 只监听 workspace 根与 `.svn` 元数据目录（svn 操作会改写其中的 wc.db）。
 */
export function buildSvnAutoRefreshWatchPaths(
  workspacePath: string,
  wcRoot: string | undefined,
  platform: string | null,
): FileWatchRefreshPath[] {
  if (!platform) {
    return [];
  }
  if (platform !== "linux") {
    return [{ path: workspacePath, recursive: true }];
  }
  const paths: FileWatchRefreshPath[] = [{ path: workspacePath, recursive: false }];
  if (wcRoot) {
    paths.push({ path: joinFilePath(wcRoot, ".svn"), recursive: false });
  }
  return paths;
}

/**
 * workspace 的 VCS 判定与 SVN 状态的唯一所有者（spec：docs/svn/architecture.md）。
 * 状态只镜像 Host 上 svn 命令的最近一次读取结果，不做乐观修改；
 * 请求序号丢弃过期响应，workspace 切换后旧响应不会写入新 workspace。
 */
export function useWorkspaceVcs({
  workspacePath,
  workspaceIdentity,
  remoteSessionId,
  enabled,
}: {
  workspacePath: string;
  workspaceIdentity?: string | null;
  remoteSessionId?: string | null;
  enabled: boolean;
}): WorkspaceVcsController {
  const services = useWorkspaceServices(workspacePath, remoteSessionId, workspaceIdentity);
  const workspaceVcsService = services.workspaceVcsService ?? null;
  const svnService = services.svnService ?? null;
  const { fileWatcherService, systemService } = services;
  const { settings } = useSettings();
  const workspaceKey = workspaceIdentity?.trim() || workspacePath;
  const request = useMemo<SvnRequest>(
    () => ({
      workspacePath,
      ...(workspaceIdentity?.trim() ? { workspaceIdentity: workspaceIdentity.trim() } : {}),
    }),
    [workspaceIdentity, workspacePath],
  );

  const [vcsState, setVcsState] = useState<{ key: string; info: WorkspaceVcsInfo } | null>(null);
  // 设置页修改覆盖值后只广播“已变更”，这里重新读取，判定结果仍只有 Host 一个来源。
  const [detectVersion, setDetectVersion] = useState(0);
  useEffect(
    () =>
      subscribeWorkspaceVcsChanged((changedKey) => {
        if (changedKey === workspaceKey) {
          setDetectVersion((value) => value + 1);
        }
      }),
    [workspaceKey],
  );
  const vcs = vcsState?.key === workspaceKey ? vcsState.info : null;
  const isSvn = enabled && vcs?.kind === "svn";

  useEffect(() => {
    if (!enabled || !workspacePath) {
      return;
    }
    if (!workspaceVcsService) {
      setVcsState({ key: workspaceKey, info: LEGACY_GIT_VCS_INFO });
      return;
    }
    let cancelled = false;
    void workspaceVcsService
      .getWorkspaceVcsInfo(request)
      .then((info) => {
        if (!cancelled) {
          setVcsState({ key: workspaceKey, info });
        }
      })
      .catch((error: unknown) => {
        if (cancelled) {
          return;
        }
        // 旧 Host / 旧远端 server 没有 workspace-vcs channel：保持 Git 旧行为。
        logger.info("[SvnWorkspace] VCS 判定不可用，按 Git 处理", {
          workspacePath,
          error: error instanceof Error ? error.message : String(error),
        });
        setVcsState({ key: workspaceKey, info: LEGACY_GIT_VCS_INFO });
      });
    return () => {
      cancelled = true;
    };
  }, [detectVersion, enabled, request, workspaceKey, workspacePath, workspaceVcsService]);

  const setOverride = useCallback(
    async (override: WorkspaceVcsOverride) => {
      if (!workspaceVcsService) {
        return;
      }
      const info = await workspaceVcsService.setWorkspaceVcsOverride({ ...request, override });
      setVcsState({ key: workspaceKey, info });
    },
    [request, workspaceKey, workspaceVcsService],
  );

  const [state, setState] = useState<SvnWorkspaceState>(() => createEmptyState(workspaceKey));
  const requestSeqRef = useRef(0);
  const remoteSeqRef = useRef(0);
  const currentState = state.workspaceKey === workspaceKey ? state : createEmptyState(workspaceKey);

  const refresh = useCallback(() => {
    if (!isSvn) {
      return;
    }
    const seq = ++requestSeqRef.current;
    setState((previous) =>
      previous.workspaceKey === workspaceKey
        ? { ...previous, loading: true }
        : { ...createEmptyState(workspaceKey), loading: true },
    );
    void (async () => {
      let info: SvnWorkingCopyInfo;
      try {
        info = svnService ? await svnService.getWorkingCopyInfo(request) : toUnsupportedHostInfo();
      } catch (error) {
        // Rust Host / 旧 Host 没有 svn channel：调用失败统一呈现为“当前 Host 不支持”。
        logger.info("[SvnWorkspace] svn channel 不可用", {
          workspacePath,
          error: error instanceof Error ? error.message : String(error),
        });
        info = toUnsupportedHostInfo();
      }
      let changes: SvnFileChange[] = [];
      let ignoredPaths: string[] = [];
      let failure: SvnFailure | null = null;
      if (svnService && info.availability.status === "available" && info.isWorkingCopy) {
        try {
          const result = await svnService.getChanges(request);
          if (result.ok) {
            changes = result.changes;
            ignoredPaths = result.ignoredPaths;
          } else {
            failure = result;
          }
        } catch (error) {
          failure = {
            ok: false,
            issue: "unknown",
            message: error instanceof Error ? error.message : String(error),
          };
        }
      }
      if (seq !== requestSeqRef.current) {
        return;
      }
      setState((previous) => ({
        workspaceKey,
        info,
        changes,
        ignoredPaths,
        loading: false,
        failure,
        revision: (previous.workspaceKey === workspaceKey ? previous.revision : 0) + 1,
        remote: previous.workspaceKey === workspaceKey ? previous.remote : null,
      }));
    })();
  }, [isSvn, request, svnService, workspaceKey, workspacePath]);

  const canQueryRemote =
    isSvn &&
    Boolean(svnService) &&
    currentState.info?.availability.status === "available" &&
    currentState.info.isWorkingCopy;

  const refreshRemote = useCallback(() => {
    if (!canQueryRemote || !svnService) {
      return;
    }
    const seq = ++remoteSeqRef.current;
    void svnService
      .getRemoteStatus(request)
      .then((result) => {
        if (seq !== remoteSeqRef.current) {
          return;
        }
        // 远端检查失败（未认证、网络不可达）只是不显示提示，不打断本地审阅。
        setState((previous) =>
          previous.workspaceKey === workspaceKey
            ? { ...previous, remote: result.ok ? result : null }
            : previous,
        );
      })
      .catch(() => undefined);
  }, [canQueryRemote, request, svnService, workspaceKey]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const remoteCheckEnabled = settings?.svnRemoteCheckEnabled !== false;
  useEffect(() => {
    if (!canQueryRemote || !remoteCheckEnabled) {
      return;
    }
    refreshRemote();
    const timer = setInterval(refreshRemote, SVN_REMOTE_CHECK_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [canQueryRemote, refreshRemote, remoteCheckEnabled]);

  const [platformState, setPlatformState] = useState<{
    service: typeof systemService;
    platform: string;
  } | null>(null);
  const platform = platformState?.service === systemService ? platformState.platform : null;
  useEffect(() => {
    if (!canQueryRemote) {
      return;
    }
    let cancelled = false;
    void systemService
      .info()
      .then((info) => {
        if (!cancelled) {
          setPlatformState({ service: systemService, platform: info.platform });
        }
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [canQueryRemote, systemService]);

  const wcRoot = currentState.info?.wcRoot;
  const watchPathSignature = JSON.stringify(
    canQueryRemote ? buildSvnAutoRefreshWatchPaths(workspacePath, wcRoot, platform) : [],
  );
  const watchPaths = useMemo(
    () => JSON.parse(watchPathSignature) as FileWatchRefreshPath[],
    [watchPathSignature],
  );
  useFileWatchRefresh({
    fileWatcherService,
    watchPaths,
    debounceMs: SVN_AUTO_REFRESH_DEBOUNCE_MS,
    onRefresh: refresh,
    logScope: "SvnAutoRefresh",
    workspacePath,
  });

  const svn = useMemo<SvnWorkspaceController | null>(
    () =>
      isSvn
        ? {
            workspacePath,
            request,
            svnService,
            state: currentState,
            refresh,
            refreshRemote,
          }
        : null,
    [currentState, isSvn, refresh, refreshRemote, request, svnService, workspacePath],
  );

  return { vcs: enabled ? vcs : null, setOverride, svn };
}
