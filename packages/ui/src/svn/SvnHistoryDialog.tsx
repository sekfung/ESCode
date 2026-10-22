import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { GitDiffResult, SvnCredentials, SvnLogEntry, SvnResult } from "@escode/shared";
import { TID_SVN_HISTORY_DIALOG } from "@escode/shared";
import { ArrowLeftIcon } from "lucide-react";
import { cn } from "@/components/lib/utils.js";
import { Button } from "@/components/ui/button.js";
import { DiffViewer } from "@/components/ui/diff-viewer.js";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog.js";
import { Input } from "@/components/ui/input.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import { useESCodeStore } from "@/store/StoreProvider.js";
import { resolveTheme } from "@/useTheme.js";
import type { SvnWorkspaceController } from "@/svn/useSvnWorkspace.js";
import {
  getSvnIssueMessageId,
  getSvnLogMessageTitle,
  matchesSvnLogFilter,
  toSvnDiffCardModel,
} from "@/svn/svnDisplay.js";

const PAGE_SIZE = 100;

/**
 * 单线历史（第一期不画分支拓扑，spec：docs/svn/ui.md §5）。
 * 分页从上一页最早修订的前一个修订继续读取；筛选只作用于已加载条目。
 */
export function SvnHistoryDialog({
  open,
  controller,
  branchLabel,
  targetPath,
  runWithCredentialRetry,
  onOpenChange,
}: {
  open: boolean;
  controller: SvnWorkspaceController;
  branchLabel: string | null;
  /** 提供时查看该文件的仓库 URL 历史（spec：docs/svn/ui.md §12）。 */
  targetPath?: string;
  runWithCredentialRetry: <T extends object>(
    operation: (credentials?: SvnCredentials) => Promise<SvnResult<T>>,
  ) => Promise<SvnResult<T>>;
  onOpenChange: (open: boolean) => void;
}) {
  const { intl, locale } = useESCodeIntl();
  const { svnService, request } = controller;
  const theme = useESCodeStore((store) => store.theme);
  const codePreviewSettings = useESCodeStore((store) => store.codePreviewSettings);
  const resolvedTheme = resolveTheme(theme);
  const [entries, setEntries] = useState<SvnLogEntry[]>([]);
  const [hasMore, setHasMore] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [selectedRevision, setSelectedRevision] = useState<number | null>(null);
  const [diffPath, setDiffPath] = useState<string | null>(null);
  const [diff, setDiff] = useState<{ key: string; diff: GitDiffResult | null } | null>(null);
  const loadSeqRef = useRef(0);
  const dateFormatter = useMemo(
    () => new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }),
    [locale],
  );

  const loadPage = useCallback(
    async (startRevision?: number) => {
      if (!svnService) {
        return;
      }
      const seq = ++loadSeqRef.current;
      setLoading(true);
      setError(null);
      const result = await runWithCredentialRetry((credentials) =>
        targetPath
          ? svnService.getFileLog({
              ...request,
              path: targetPath,
              limit: PAGE_SIZE,
              ...(startRevision !== undefined ? { startRevision } : {}),
              ...(credentials ? { credentials } : {}),
            })
          : svnService.getLog({
              ...request,
              limit: PAGE_SIZE,
              ...(startRevision !== undefined ? { startRevision } : {}),
              ...(credentials ? { credentials } : {}),
            }),
      ).catch(
        (loadError: unknown): SvnResult<{ entries: SvnLogEntry[]; hasMore: boolean }> => ({
          ok: false,
          issue: "unknown",
          message: loadError instanceof Error ? loadError.message : String(loadError),
        }),
      );
      if (seq !== loadSeqRef.current) {
        return;
      }
      setLoading(false);
      if (!result.ok) {
        if (result.issue !== "cancelled") {
          setError(
            intl.formatMessage(
              { id: getSvnIssueMessageId(result.issue) },
              { message: result.message },
            ),
          );
        }
        return;
      }
      setEntries((previous) =>
        startRevision === undefined ? result.entries : [...previous, ...result.entries],
      );
      setHasMore(result.hasMore);
      if (startRevision === undefined) {
        setSelectedRevision(result.entries[0]?.revision ?? null);
      }
    },
    [intl, request, runWithCredentialRetry, svnService, targetPath],
  );

  useEffect(() => {
    if (!open) {
      return;
    }
    setEntries([]);
    setFilter("");
    setSelectedRevision(null);
    setDiffPath(null);
    void loadPage();
  }, [loadPage, open]);

  const visibleEntries = useMemo(
    () => entries.filter((entry) => matchesSvnLogFilter(entry, filter)),
    [entries, filter],
  );
  const selectedEntry = entries.find((entry) => entry.revision === selectedRevision) ?? null;

  useEffect(() => {
    if (!svnService || !selectedEntry || !diffPath) {
      return;
    }
    const key = `${selectedEntry.revision}:${diffPath}`;
    let cancelled = false;
    setDiff({ key, diff: null });
    void svnService
      .getDiff({ ...request, path: diffPath, revision: selectedEntry.revision })
      .then((result) => {
        if (!cancelled) {
          setDiff({
            key,
            diff: result.ok ? toSvnDiffCardModel(result, result.patch ?? undefined) : null,
          });
        }
      })
      .catch(() => {
        if (!cancelled) {
          setDiff({ key, diff: null });
        }
      });
    return () => {
      cancelled = true;
    };
  }, [diffPath, request, selectedEntry, svnService]);

  const lastRevision = entries[entries.length - 1]?.revision;
  const showDetailOnly = selectedEntry !== null && diffPath !== null;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        data-testid={TID_SVN_HISTORY_DIALOG}
        className="flex h-[min(78vh,720px)] max-w-5xl flex-col gap-0 overflow-hidden rounded-2xl p-0"
      >
        <DialogHeader className="border-b border-border px-5 py-4">
          <DialogTitle className="text-ui-lg font-medium text-foreground">
            {intl.formatMessage(
              { id: "svn.history.title" },
              { branch: branchLabel ?? intl.formatMessage({ id: "svn.pane.title" }) },
            )}
          </DialogTitle>
        </DialogHeader>
        <div className="flex min-h-0 flex-1">
          <div
            className={cn(
              "flex min-h-0 w-full flex-col border-border md:w-80 md:shrink-0 md:border-r",
              selectedEntry && "hidden md:flex",
            )}
          >
            <div className="space-y-1 p-3">
              <Input
                value={filter}
                placeholder={intl.formatMessage({ id: "svn.history.filterPlaceholder" })}
                onChange={(event) => setFilter(event.target.value)}
              />
              <p className="text-ui-sm text-foreground-subtle">
                {intl.formatMessage({ id: "svn.history.filterScope" }, { count: entries.length })}
              </p>
            </div>
            <div className="min-h-0 flex-1 overflow-auto px-1 pb-2">
              {visibleEntries.map((entry) => (
                <button
                  key={entry.revision}
                  type="button"
                  data-svn-revision={entry.revision}
                  className={cn(
                    "flex w-full flex-col gap-0.5 rounded-lg px-3 py-2 text-left hover:bg-hover",
                    entry.revision === selectedRevision && "bg-selected",
                  )}
                  onClick={() => {
                    setSelectedRevision(entry.revision);
                    setDiffPath(null);
                  }}
                >
                  <span className="truncate text-ui-base text-foreground">
                    {getSvnLogMessageTitle(entry.message) ||
                      intl.formatMessage({ id: "svn.history.noMessage" })}
                  </span>
                  <span className="truncate text-ui-sm text-foreground-subtle">
                    <span className="font-mono">r{entry.revision}</span> · {entry.author} ·{" "}
                    {entry.date ? dateFormatter.format(new Date(entry.date)) : ""}
                  </span>
                </button>
              ))}
              {error ? <p className="px-3 py-2 text-ui-base text-destructive">{error}</p> : null}
              {loading ? (
                <p className="px-3 py-2 text-ui-base text-foreground-subtle">
                  {intl.formatMessage({ id: "common.loading" })}
                </p>
              ) : hasMore && lastRevision !== undefined && lastRevision > 1 ? (
                <Button
                  type="button"
                  variant="ghost"
                  className="mx-2 mt-1"
                  onClick={() => void loadPage(lastRevision - 1)}
                >
                  {intl.formatMessage({ id: "svn.history.loadMore" })}
                </Button>
              ) : null}
            </div>
          </div>
          <div
            className={cn(
              "min-h-0 min-w-0 flex-1 flex-col",
              selectedEntry ? "flex" : "hidden md:flex",
            )}
          >
            {selectedEntry ? (
              <div className="min-h-0 flex-1 overflow-auto p-4">
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  className={cn("mb-2", !showDetailOnly && "md:hidden")}
                  onClick={() => (diffPath ? setDiffPath(null) : setSelectedRevision(null))}
                >
                  <ArrowLeftIcon />
                  {intl.formatMessage({ id: "svn.history.back" })}
                </Button>
                <p className="text-ui-sm text-foreground-subtle">
                  <span className="font-mono">r{selectedEntry.revision}</span> ·{" "}
                  {selectedEntry.author} ·{" "}
                  {selectedEntry.date ? dateFormatter.format(new Date(selectedEntry.date)) : ""}
                </p>
                <p className="mt-2 whitespace-pre-wrap text-ui-base text-foreground">
                  {selectedEntry.message}
                </p>
                <div className="mt-4 space-y-0.5">
                  {selectedEntry.changedPaths.map((changedPath) => (
                    <button
                      key={`${changedPath.action}:${changedPath.path}`}
                      type="button"
                      disabled={changedPath.kind === "dir"}
                      className={cn(
                        "flex w-full min-w-0 items-center gap-2 rounded-md px-2 py-1 text-left text-ui-base hover:bg-hover disabled:hover:bg-transparent",
                        diffPath === changedPath.path && "bg-selected",
                      )}
                      onClick={() => setDiffPath(changedPath.path)}
                    >
                      <span className="w-3 shrink-0 font-mono text-foreground-subtle">
                        {changedPath.action}
                      </span>
                      <span className="min-w-0 truncate text-foreground">{changedPath.path}</span>
                      {changedPath.copyFromPath ? (
                        <span className="shrink-0 truncate text-ui-sm text-foreground-subtle">
                          {intl.formatMessage(
                            { id: "svn.history.copiedFrom" },
                            {
                              path: changedPath.copyFromPath,
                              revision: changedPath.copyFromRevision ?? "",
                            },
                          )}
                        </span>
                      ) : null}
                    </button>
                  ))}
                </div>
                {diffPath ? (
                  <div className="mt-4 overflow-x-auto rounded-lg border border-border">
                    {diff?.key !== `${selectedEntry.revision}:${diffPath}` || !diff ? (
                      <p className="px-4 py-3 text-ui-base text-foreground-subtle">
                        {intl.formatMessage({ id: "common.loading" })}
                      </p>
                    ) : diff.diff?.availability === "patch" && diff.diff.patch ? (
                      <DiffViewer
                        patch={diff.diff.patch}
                        diffClassName="block"
                        fontSizePx={codePreviewSettings.fontSizePx}
                        lightTheme={codePreviewSettings.lightTheme}
                        darkTheme={codePreviewSettings.darkTheme}
                        themeType={resolvedTheme}
                      />
                    ) : (
                      <p className="px-4 py-3 text-ui-base text-foreground-subtle">
                        {intl.formatMessage({
                          id:
                            diff.diff?.availability === "binary"
                              ? "git.diff.binaryTitle"
                              : "git.diff.unavailableTitle",
                        })}
                      </p>
                    )}
                  </div>
                ) : null}
              </div>
            ) : (
              <div className="flex flex-1 items-center justify-center text-ui-base text-foreground-subtle">
                {loading ? intl.formatMessage({ id: "common.loading" }) : null}
              </div>
            )}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
