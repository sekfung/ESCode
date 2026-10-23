/* eslint-disable max-lines -- SVN 面板集中承载分组列表、diff 懒加载、勾选与工作副本操作；结构与 GitPane 对齐。 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type {
  GitDiffResult,
  SvnFailure,
  SvnFileChange,
  SvnMergeResult,
  SvnResolveAccept,
  SvnResult,
} from "@escode/shared";
import {
  TID_SVN_COMMIT_BUTTON,
  TID_SVN_BRANCH_GRAPH_BUTTON,
  TID_SVN_EXTERNALS_BUTTON,
  TID_SVN_MERGE_BUTTON,
  TID_SVN_PANE,
  TID_SVN_REFRESH_BUTTON,
  TID_SVN_SHELVES_BUTTON,
  TID_SVN_UPDATE_BUTTON,
} from "@escode/shared";
import {
  AlertTriangleIcon,
  ArchiveIcon,
  ChevronDownIcon,
  DownloadIcon,
  FileTextIcon,
  GitMergeIcon,
  HistoryIcon,
  LinkIcon,
  NetworkIcon,
  RefreshCw,
  Undo2Icon,
} from "lucide-react";
import { cn } from "@/components/lib/utils.js";
import { Button } from "@/components/ui/button.js";
import { toast } from "@/components/ui/toast.js";
import type { GitPaneFileChange } from "@/hooks/useGitRepository.js";
import { useConfirmDialog } from "@/hooks/useConfirmDialog.js";
import { useFileContextActions } from "@/hooks/useFileContextActions.js";
import { useWorkspaceOpenInEditorTarget } from "@/hooks/useWorkspaceOpenInEditorTarget.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import { logger } from "@/logger.js";
import { useESCodeStore } from "@/store/StoreProvider.js";
import { resolveTheme } from "@/useTheme.js";
import { getWorkspaceFileRelativePath } from "@/workspace-file-tree/model.js";
import { SvnChangeRow } from "@/svn/SvnChangeRow.js";
import { SvnBlameDialog } from "@/svn/SvnBlameDialog.js";
import { SvnBranchGraphDialog } from "@/svn/SvnBranchGraphDialog.js";
import { SvnCommitDialog } from "@/svn/SvnCommitDialog.js";
import { SvnExternalsDialog } from "@/svn/SvnExternalsDialog.js";
import { SvnHistoryDialog } from "@/svn/SvnHistoryDialog.js";
import { SvnMergeDialog } from "@/svn/SvnMergeDialog.js";
import { SvnShelvesDialog } from "@/svn/SvnShelvesDialog.js";
import { SvnUnavailableState } from "@/svn/SvnUnavailableState.js";
import { useSvnCredentialPrompt } from "@/svn/useSvnCredentialPrompt.js";
import type { SvnWorkspaceController } from "@/svn/useSvnWorkspace.js";
import {
  getSelectedSvnPaths,
  getSvnBranchLabel,
  getSvnCommitBlockReason,
  getSvnIssueMessageId,
  groupSvnChanges,
  reconcileSvnSelection,
  toSvnChangeCardModel,
  toSvnDiffCardModel,
  type SvnChangeGroupId,
} from "@/svn/svnDisplay.js";

interface DiffLoadState {
  loading: boolean;
  diff: GitDiffResult | null;
}

export function SvnPane({
  controller,
  workspaceIdentity,
  workspaceRemoteSessionId,
  isDesktop,
  onOpenFile,
  onRevealFileInTree,
}: {
  controller: SvnWorkspaceController;
  workspaceIdentity?: string;
  workspaceRemoteSessionId?: string;
  isDesktop?: boolean;
  onOpenFile?: (absolutePath: string) => void;
  onRevealFileInTree?: (path: string) => void;
}) {
  const { intl } = useESCodeIntl();
  const { state, svnService, request, workspacePath, refresh, refreshRemote } = controller;
  const theme = useESCodeStore((store) => store.theme);
  const codePreviewSettings = useESCodeStore((store) => store.codePreviewSettings);
  const resolvedTheme = resolveTheme(theme);
  const confirm = useConfirmDialog();
  const workspaceOpenTarget = useWorkspaceOpenInEditorTarget({
    workspacePath,
    workspaceIdentity,
    workspaceRemoteSessionId,
  });
  const fileActions = useFileContextActions({
    canOpenLocalFileManager: Boolean(isDesktop),
    isRemoteWorkspace:
      Boolean(workspaceRemoteSessionId || workspaceIdentity?.trim()) ||
      workspaceOpenTarget.isRemoteWorkspace,
    remoteTarget: workspaceOpenTarget.remoteTarget,
    workspaceIdentity,
  });
  const { runWithCredentialRetry, credentialDialog } = useSvnCredentialPrompt(state.info?.url);

  const [selection, setSelection] = useState<Map<string, boolean>>(new Map());
  const [expandedPath, setExpandedPath] = useState<string | null>(null);
  const [collapsedGroups, setCollapsedGroups] = useState<Set<SvnChangeGroupId>>(
    () => new Set(["unversioned"]),
  );
  const [diffByPath, setDiffByPath] = useState<Record<string, DiffLoadState>>({});
  const [pendingAction, setPendingAction] = useState<string | null>(null);
  const [lockedFailure, setLockedFailure] = useState<SvnFailure | null>(null);
  const [commitDialogOpen, setCommitDialogOpen] = useState(false);
  const [historyOpen, setHistoryOpen] = useState(false);
  const [mergeDialogOpen, setMergeDialogOpen] = useState(false);
  const [fileHistoryPath, setFileHistoryPath] = useState<string | null>(null);
  const [blamePath, setBlamePath] = useState<string | null>(null);
  const [branchGraphOpen, setBranchGraphOpen] = useState(false);
  const [externalsOpen, setExternalsOpen] = useState(false);
  const [shelvesOpen, setShelvesOpen] = useState(false);
  const diffGenerationRef = useRef(0);

  const groups = useMemo(() => groupSvnChanges(state.changes), [state.changes]);
  const selectedPaths = useMemo(() => getSelectedSvnPaths(selection), [selection]);
  const commitBlockReason = getSvnCommitBlockReason(state.changes, selectedPaths);
  const busy = pendingAction !== null || state.loading;
  const info = state.info;
  const branchLabel = getSvnBranchLabel(info?.branch);
  const isLocked =
    lockedFailure?.issue === "working-copy-locked" ||
    state.failure?.issue === "working-copy-locked";

  useEffect(() => {
    setSelection((previous) => reconcileSvnSelection(previous, state.changes));
    diffGenerationRef.current += 1;
    setDiffByPath({});
  }, [state.changes, state.revision]);

  const cardModels = useMemo(
    () =>
      new Map(
        state.changes.map((change) => [change.path, toSvnChangeCardModel(workspacePath, change)]),
      ),
    [state.changes, workspacePath],
  );

  const reportFailure = useCallback(
    (result: SvnFailure) => {
      if (result.issue === "cancelled") {
        return;
      }
      if (result.issue === "working-copy-locked") {
        setLockedFailure(result);
      }
      toast(
        intl.formatMessage({ id: getSvnIssueMessageId(result.issue) }, { message: result.message }),
        { variant: "warning" },
      );
    },
    [intl],
  );

  /** 写操作统一入口：执行 → 失败提示 → 无论成败都主动刷新（不依赖文件事件）。 */
  const runAction = useCallback(
    async <T extends object>(
      actionId: string,
      operation: () => Promise<SvnResult<T>>,
    ): Promise<SvnResult<T> | null> => {
      if (!svnService) {
        return null;
      }
      setPendingAction(actionId);
      try {
        const result = await operation();
        if (result.ok) {
          setLockedFailure(null);
        } else {
          reportFailure(result);
        }
        return result;
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        logger.warn("[SvnPane] SVN 操作失败", { workspacePath, actionId, error: message });
        reportFailure({ ok: false, issue: "unknown", message });
        return null;
      } finally {
        setPendingAction(null);
        refresh();
      }
    },
    [refresh, reportFailure, svnService, workspacePath],
  );

  const loadDiff = useCallback(
    (path: string) => {
      if (!svnService || diffByPath[path]) {
        return;
      }
      const generation = diffGenerationRef.current;
      setDiffByPath((current) => ({ ...current, [path]: { loading: true, diff: null } }));
      void svnService
        .getDiff({ ...request, path })
        .then((result) => {
          if (generation !== diffGenerationRef.current) {
            return;
          }
          setDiffByPath((current) => ({
            ...current,
            [path]: {
              loading: false,
              diff: result.ok
                ? toSvnDiffCardModel(result)
                : {
                    path,
                    availability: "unavailable",
                    patch: null,
                    beforeContent: null,
                    afterContent: null,
                    summary: result.message,
                  },
            },
          }));
        })
        .catch((error: unknown) => {
          if (generation !== diffGenerationRef.current) {
            return;
          }
          setDiffByPath((current) => ({
            ...current,
            [path]: {
              loading: false,
              diff: {
                path,
                availability: "unavailable",
                patch: null,
                beforeContent: null,
                afterContent: null,
                summary: error instanceof Error ? error.message : String(error),
              },
            },
          }));
        });
    },
    [diffByPath, request, svnService],
  );

  const handleOpenChange = useCallback(
    (change: GitPaneFileChange, nextOpen: boolean) => {
      const path = change.repoRelativePath;
      setExpandedPath(nextOpen ? path : null);
      if (nextOpen) {
        loadDiff(path);
      }
    },
    [loadDiff],
  );

  const handleCheckedChange = useCallback((path: string, checked: boolean) => {
    setSelection((previous) => new Map(previous).set(path, checked));
  }, []);

  const handleUpdate = useCallback(async () => {
    const result = await runAction("update", () =>
      runWithCredentialRetry((credentials) =>
        svnService!.update({ ...request, ...(credentials ? { credentials } : {}) }),
      ),
    );
    if (result?.ok) {
      toast(
        intl.formatMessage(
          {
            id: result.conflictCount > 0 ? "svn.toast.updatedWithConflicts" : "svn.toast.updated",
          },
          {
            revision: result.revision,
            count: result.changedFileCount,
            conflicts: result.conflictCount,
          },
        ),
        result.conflictCount > 0 ? { variant: "warning" } : undefined,
      );
      if (result.conflictCount > 0) {
        setCollapsedGroups((previous) => {
          const next = new Set(previous);
          next.delete("conflicted");
          return next;
        });
      }
      refreshRemote();
    }
  }, [intl, refreshRemote, request, runAction, runWithCredentialRetry, svnService]);

  const handleResolve = useCallback(
    async (path: string, accept: SvnResolveAccept) => {
      if (accept !== "working") {
        const confirmed = await confirm({
          title: intl.formatMessage({ id: "svn.conflict.confirmTitle" }),
          description: intl.formatMessage(
            {
              id:
                accept === "mine-full"
                  ? "svn.conflict.confirmUseMine"
                  : "svn.conflict.confirmUseTheirs",
            },
            { path },
          ),
          confirmLabel: intl.formatMessage({
            id: accept === "mine-full" ? "svn.conflict.useMine" : "svn.conflict.useTheirs",
          }),
        });
        if (!confirmed) {
          return;
        }
      }
      await runAction(`resolve:${path}`, () => svnService!.resolve({ ...request, path, accept }));
    },
    [confirm, intl, request, runAction, svnService],
  );

  const handleResolveRow = useCallback(
    (path: string, accept: SvnResolveAccept) => {
      void handleResolve(path, accept);
    },
    [handleResolve],
  );

  const handleRevert = useCallback(async () => {
    const paths = selectedPaths.filter(
      (path) => state.changes.find((change) => change.path === path)?.status !== "unversioned",
    );
    if (paths.length === 0) {
      return;
    }
    const confirmed = await confirm({
      title: intl.formatMessage({ id: "svn.revert.confirmTitle" }, { count: paths.length }),
      description: intl.formatMessage(
        { id: "svn.revert.confirmDescription" },
        { paths: paths.slice(0, 10).join("\n") },
      ),
      confirmLabel: intl.formatMessage({ id: "svn.action.revert" }),
    });
    if (!confirmed) {
      return;
    }
    await runAction("revert", () => svnService!.revert({ ...request, paths }));
  }, [confirm, intl, request, runAction, selectedPaths, state.changes, svnService]);

  const handleCleanup = useCallback(async () => {
    const result = await runAction("cleanup", () => svnService!.cleanup(request));
    if (result?.ok) {
      toast(intl.formatMessage({ id: "svn.toast.cleanedUp" }));
    }
  }, [intl, request, runAction, svnService]);

  const handleMerged = useCallback((result: SvnMergeResult) => {
    if (result.conflictCount > 0) {
      setCollapsedGroups((previous) => {
        const next = new Set(previous);
        next.delete("conflicted");
        return next;
      });
    }
  }, []);

  const handleShowFileHistory = useCallback((change: SvnFileChange) => {
    setFileHistoryPath(change.path);
  }, []);

  const handleShowBlame = useCallback((change: SvnFileChange) => {
    setBlamePath(change.path);
  }, []);

  const handleLock = useCallback(
    async (change: SvnFileChange) => {
      const result = await runAction(`lock:${change.path}`, () =>
        svnService!.lock({ ...request, paths: [change.path] }),
      );
      if (result?.ok) {
        toast(intl.formatMessage({ id: "svn.lock.locked" }, { path: change.path }));
      }
    },
    [intl, request, runAction, svnService],
  );

  const handleUnlock = useCallback(
    async (change: SvnFileChange) => {
      const confirmed = await confirm({
        title: intl.formatMessage({ id: "svn.lock.confirmTitle" }, { path: change.path }),
        description: intl.formatMessage(
          { id: "svn.lock.confirmDescription" },
          { owner: change.lock?.owner ?? "" },
        ),
        confirmLabel: intl.formatMessage({ id: "svn.lock.unlock" }),
      });
      if (!confirmed) {
        return;
      }
      const result = await runAction(`unlock:${change.path}`, () =>
        svnService!.unlock({ ...request, paths: [change.path], force: true }),
      );
      if (result?.ok) {
        toast(intl.formatMessage({ id: "svn.lock.unlocked" }, { path: change.path }));
      }
    },
    [confirm, intl, request, runAction, svnService],
  );

  const resolveChangePath = useCallback((change: GitPaneFileChange) => change.path, []);
  const contextMenuLabels = useMemo(
    () => ({
      copyAbsolutePath: intl.formatMessage({ id: "fileActions.copyAbsolutePath" }),
      copyRelativePath: intl.formatMessage({ id: "fileActions.copyRelativePath" }),
      revealInFileManager: intl.formatMessage({ id: "git.changeContext.revealInFileManager" }),
      revealInFileTree: intl.formatMessage({ id: "git.changeContext.revealInFileTree" }),
    }),
    [intl],
  );
  const handleCopyAbsolutePath = useCallback(
    (change: GitPaneFileChange) => void fileActions.copyAbsolutePath({ path: change.path }),
    [fileActions],
  );
  const handleCopyRelativePath = useCallback(
    (change: GitPaneFileChange) =>
      void fileActions.copyRelativePath({
        path: change.path,
        relativePath: getWorkspaceFileRelativePath(workspacePath, change.path),
      }),
    [fileActions, workspacePath],
  );
  const handleRevealInFileManager = useCallback(
    (change: GitPaneFileChange) =>
      void fileActions.revealInFileManager({
        path: resolveChangePath(change),
        deleted: change.kind === "deleted",
        kind: "file",
      }),
    [fileActions, resolveChangePath],
  );
  const handleRevealInFileTree = useCallback(
    (change: GitPaneFileChange) => onRevealFileInTree?.(change.path),
    [onRevealFileInTree],
  );
  const handleOpenFile = useCallback(
    (change: GitPaneFileChange) => onOpenFile?.(change.path),
    [onOpenFile],
  );

  if (!info || info.availability.status !== "available" || !info.isWorkingCopy) {
    return (
      <section data-testid={TID_SVN_PANE} className="flex h-full min-h-0 flex-col bg-background">
        <SvnUnavailableState
          info={info}
          loading={state.loading}
          svnService={svnService}
          onRefresh={refresh}
        />
      </section>
    );
  }

  const remoteOutOfDate = state.remote && state.remote.outOfDateCount > 0 ? state.remote : null;

  return (
    <section data-testid={TID_SVN_PANE} className="flex h-full min-h-0 flex-col bg-background">
      <div className="flex items-center justify-between gap-2 p-3">
        <div className="min-w-0 flex-1">
          <p className="truncate text-ui-base font-medium text-foreground">
            {branchLabel ?? intl.formatMessage({ id: "svn.pane.title" })}
            {info.revision !== undefined ? (
              <span className="ml-2 font-mono text-foreground-subtle">r{info.revision}</span>
            ) : null}
          </p>
          {remoteOutOfDate ? (
            <button
              type="button"
              className="truncate text-ui-sm text-primary hover:underline disabled:opacity-50"
              disabled={busy}
              onClick={() => void handleUpdate()}
            >
              {intl.formatMessage(
                { id: "svn.pane.serverHasNewRevision" },
                { revision: remoteOutOfDate.headRevision },
              )}
            </button>
          ) : null}
        </div>
        <div className="flex shrink-0 items-center gap-1">
          <Button
            type="button"
            data-testid={TID_SVN_UPDATE_BUTTON}
            variant="ghost"
            size="lg"
            disabled={busy}
            onClick={() => void handleUpdate()}
          >
            <DownloadIcon
              className={cn("size-3.5", pendingAction === "update" && "animate-pulse")}
            />
            {intl.formatMessage({ id: "svn.action.update" })}
          </Button>
          <Button
            type="button"
            data-testid={TID_SVN_MERGE_BUTTON}
            variant="ghost"
            size="lg"
            disabled={busy || !info.canSwitch}
            title={
              info.canSwitch ? undefined : intl.formatMessage({ id: "svn.merge.cannotSwitch" })
            }
            onClick={() => setMergeDialogOpen(true)}
          >
            <GitMergeIcon className="size-3.5" />
            {intl.formatMessage({ id: "svn.action.merge" })}
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon-lg"
            aria-label={intl.formatMessage({ id: "svn.action.history" })}
            onClick={() => setHistoryOpen(true)}
          >
            <HistoryIcon className="size-4" />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon-lg"
            data-testid={TID_SVN_BRANCH_GRAPH_BUTTON}
            aria-label={intl.formatMessage({ id: "svn.action.branchGraph" })}
            onClick={() => setBranchGraphOpen(true)}
          >
            <NetworkIcon className="size-4" />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon-lg"
            data-testid={TID_SVN_EXTERNALS_BUTTON}
            aria-label={intl.formatMessage({ id: "svn.action.externals" })}
            onClick={() => setExternalsOpen(true)}
          >
            <LinkIcon className="size-4" />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon-lg"
            data-testid={TID_SVN_SHELVES_BUTTON}
            aria-label={intl.formatMessage({ id: "svn.action.shelves" })}
            onClick={() => setShelvesOpen(true)}
          >
            <ArchiveIcon className="size-4" />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon-lg"
            data-testid={TID_SVN_REFRESH_BUTTON}
            aria-label={intl.formatMessage({ id: "git.action.refresh" })}
            disabled={state.loading}
            onClick={refresh}
          >
            <RefreshCw className={cn("size-3.5", state.loading && "animate-spin")} />
          </Button>
        </div>
      </div>

      {isLocked ? (
        <div className="mx-3 mb-2 flex items-center gap-2 rounded-lg bg-warning/10 px-3 py-2">
          <AlertTriangleIcon className="size-4 shrink-0 text-warning" />
          <p className="min-w-0 flex-1 text-ui-base text-foreground">
            {intl.formatMessage({ id: "svn.issue.working-copy-locked" }, { message: "" })}
          </p>
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={busy}
            onClick={() => void handleCleanup()}
          >
            {intl.formatMessage({ id: "svn.action.cleanup" })}
          </Button>
        </div>
      ) : null}

      {state.failure && state.failure.issue !== "working-copy-locked" ? (
        <p className="mx-3 mb-2 rounded-lg bg-destructive/10 px-3 py-2 text-ui-base text-destructive">
          {intl.formatMessage(
            { id: getSvnIssueMessageId(state.failure.issue) },
            { message: state.failure.message },
          )}
        </p>
      ) : null}

      <div className="min-h-0 flex-1 overflow-auto">
        {groups.length > 0 ? (
          groups.map((group) => {
            const collapsed = collapsedGroups.has(group.id);
            return (
              <div key={group.id} data-svn-group={group.id}>
                <button
                  type="button"
                  aria-expanded={!collapsed}
                  className="flex h-8 w-full items-center gap-2 px-3 text-left text-ui-sm font-medium text-foreground-subtle hover:bg-surface-hover"
                  onClick={() =>
                    setCollapsedGroups((previous) => {
                      const next = new Set(previous);
                      if (collapsed) {
                        next.delete(group.id);
                      } else {
                        next.add(group.id);
                      }
                      return next;
                    })
                  }
                >
                  <ChevronDownIcon
                    className={cn("size-3.5 transition-transform", collapsed && "-rotate-90")}
                  />
                  {intl.formatMessage(
                    { id: `svn.group.${group.id}` },
                    { count: group.changes.length },
                  )}
                </button>
                {collapsed
                  ? null
                  : group.changes.map((change) => {
                      const diffState = diffByPath[change.path];
                      const cardModel = cardModels.get(change.path)!;
                      return (
                        <SvnChangeRow
                          key={change.path}
                          change={change}
                          cardModel={cardModel}
                          checked={selection.get(change.path) ?? false}
                          disabled={busy}
                          diffState={diffState?.diff ?? null}
                          isDiffLoading={
                            expandedPath === change.path && (diffState?.loading ?? true)
                          }
                          isExpanded={expandedPath === change.path}
                          codePreviewSettings={codePreviewSettings}
                          resolvedTheme={resolvedTheme}
                          contextMenuLabels={contextMenuLabels}
                          canRevealInFileManager={fileActions.canRevealInFileManager({
                            path: cardModel.path,
                            deleted: cardModel.kind === "deleted",
                          })}
                          onCheckedChange={handleCheckedChange}
                          onOpenChange={handleOpenChange}
                          onResolve={handleResolveRow}
                          onShowFileHistory={handleShowFileHistory}
                          onShowBlame={handleShowBlame}
                          onLock={handleLock}
                          onUnlock={handleUnlock}
                          onOpenFile={handleOpenFile}
                          onCopyAbsolutePath={handleCopyAbsolutePath}
                          onCopyRelativePath={handleCopyRelativePath}
                          onRevealInFileManager={handleRevealInFileManager}
                          onRevealInFileTree={
                            onRevealFileInTree ? handleRevealInFileTree : undefined
                          }
                        />
                      );
                    })}
              </div>
            );
          })
        ) : (
          <div className="flex h-full flex-col items-center justify-center px-6 text-center">
            <FileTextIcon className="size-8 text-foreground-subtlest" />
            <p className="mt-3 text-ui-base font-medium text-foreground">
              {intl.formatMessage({ id: state.loading ? "common.loading" : "svn.pane.emptyTitle" })}
            </p>
            {state.loading ? null : (
              <p className="mt-1 text-ui-base text-foreground-subtle">
                {intl.formatMessage({ id: "svn.pane.emptyDescription" })}
              </p>
            )}
          </div>
        )}
      </div>

      {state.changes.length > 0 ? (
        <div className="flex items-center justify-between gap-2 border-t border-border p-3">
          <Button
            type="button"
            data-testid={TID_SVN_COMMIT_BUTTON}
            size="lg"
            disabled={busy || commitBlockReason !== null}
            title={
              commitBlockReason
                ? intl.formatMessage({ id: `svn.commit.blocked.${commitBlockReason}` })
                : undefined
            }
            onClick={() => setCommitDialogOpen(true)}
          >
            {intl.formatMessage({ id: "svn.action.commitCount" }, { count: selectedPaths.length })}
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="lg"
            disabled={busy || selectedPaths.length === 0}
            onClick={() => void handleRevert()}
          >
            <Undo2Icon className="size-3.5" />
            {intl.formatMessage({ id: "svn.action.revertSelected" })}
          </Button>
        </div>
      ) : null}

      <SvnCommitDialog
        open={commitDialogOpen}
        controller={controller}
        selectedPaths={selectedPaths}
        runWithCredentialRetry={runWithCredentialRetry}
        onOpenChange={setCommitDialogOpen}
        onRequestUpdate={() => {
          setCommitDialogOpen(false);
          void handleUpdate();
        }}
      />
      <SvnHistoryDialog
        open={historyOpen}
        controller={controller}
        branchLabel={branchLabel}
        runWithCredentialRetry={runWithCredentialRetry}
        onOpenChange={setHistoryOpen}
      />
      <SvnHistoryDialog
        open={fileHistoryPath !== null}
        controller={controller}
        branchLabel={fileHistoryPath}
        targetPath={fileHistoryPath ?? undefined}
        runWithCredentialRetry={runWithCredentialRetry}
        onOpenChange={(next) => {
          if (!next) {
            setFileHistoryPath(null);
          }
        }}
      />
      <SvnBlameDialog
        open={blamePath !== null}
        path={blamePath ?? ""}
        controller={controller}
        runWithCredentialRetry={runWithCredentialRetry}
        onOpenChange={(next) => {
          if (!next) {
            setBlamePath(null);
          }
        }}
      />
      <SvnBranchGraphDialog
        open={branchGraphOpen}
        controller={controller}
        runWithCredentialRetry={runWithCredentialRetry}
        onOpenChange={setBranchGraphOpen}
      />
      <SvnExternalsDialog
        open={externalsOpen}
        controller={controller}
        runWithCredentialRetry={runWithCredentialRetry}
        onOpenChange={setExternalsOpen}
      />
      <SvnShelvesDialog
        open={shelvesOpen}
        controller={controller}
        selectedPaths={selectedPaths}
        runWithCredentialRetry={runWithCredentialRetry}
        onOpenChange={setShelvesOpen}
      />
      <SvnMergeDialog
        open={mergeDialogOpen}
        controller={controller}
        runWithCredentialRetry={runWithCredentialRetry}
        onOpenChange={setMergeDialogOpen}
        onMerged={handleMerged}
      />
      {credentialDialog}
    </section>
  );
}
