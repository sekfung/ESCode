import { useCallback, useState, type FormEvent } from "react";
import type { SvnBranchListResult, SvnBranchRef } from "@escode/shared";
import { TID_SVN_BRANCH_SWITCHER } from "@escode/shared";
import { CheckIcon, ChevronDownIcon, GitBranchIcon, LoaderIcon, TagIcon } from "lucide-react";
import { cn } from "@/components/lib/utils.js";
import { Button } from "@/components/ui/button.js";
import { Input } from "@/components/ui/input.js";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover.js";
import { toast } from "@/components/ui/toast.js";
import { useConfirmDialog } from "@/hooks/useConfirmDialog.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import type { SvnWorkspaceController } from "@/svn/useSvnWorkspace.js";
import { useSvnCredentialPrompt } from "@/svn/useSvnCredentialPrompt.js";
import { getSvnBranchLabel, getSvnIssueMessageId } from "@/svn/svnDisplay.js";

/**
 * SVN 分支入口：显示当前分支并通过 svn switch 切换（spec：docs/svn/ui.md §2）。
 * 第一期不提供创建分支；非标准布局时提供“输入路径切换”。
 */
export function SvnBranchSwitcher({
  controller,
  className,
  triggerClassName,
  popoverSide = "top",
}: {
  controller: SvnWorkspaceController;
  className?: string;
  triggerClassName?: string;
  popoverSide?: "top" | "bottom" | "left" | "right";
}) {
  const { intl } = useESCodeIntl();
  const confirm = useConfirmDialog();
  const { svnService, request, state, refresh, refreshRemote } = controller;
  const info = state.info;
  const { runWithCredentialRetry, credentialDialog } = useSvnCredentialPrompt(info?.url);
  const [open, setOpen] = useState(false);
  const [branches, setBranches] = useState<SvnBranchListResult | null>(null);
  const [loading, setLoading] = useState(false);
  const [switching, setSwitching] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [customPath, setCustomPath] = useState("");

  const loadBranches = useCallback(async () => {
    if (!svnService) {
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const result = await runWithCredentialRetry((credentials) =>
        svnService.listBranches({ ...request, ...(credentials ? { credentials } : {}) }),
      );
      if (result.ok) {
        setBranches(result);
      } else if (result.issue !== "cancelled") {
        setError(
          intl.formatMessage(
            { id: getSvnIssueMessageId(result.issue) },
            { message: result.message },
          ),
        );
      }
    } catch (loadError) {
      setError(loadError instanceof Error ? loadError.message : String(loadError));
    } finally {
      setLoading(false);
    }
  }, [intl, request, runWithCredentialRetry, svnService]);

  const switchTo = useCallback(
    async (relativePath: string) => {
      if (!svnService) {
        return;
      }
      if (state.changes.some((change) => change.status !== "unversioned")) {
        const confirmed = await confirm({
          title: intl.formatMessage({ id: "svn.branch.dirtyConfirmTitle" }),
          description: intl.formatMessage({ id: "svn.branch.dirtyConfirmDescription" }),
          confirmLabel: intl.formatMessage({ id: "svn.action.switch" }),
        });
        if (!confirmed) {
          return;
        }
      }
      setOpen(false);
      setSwitching(true);
      try {
        const result = await runWithCredentialRetry((credentials) =>
          svnService.switchBranch({
            ...request,
            relativePath,
            ...(credentials ? { credentials } : {}),
          }),
        );
        if (result.ok) {
          toast(
            intl.formatMessage(
              {
                id:
                  result.conflictCount > 0
                    ? "svn.toast.switchedWithConflicts"
                    : "svn.toast.switched",
              },
              {
                branch: getSvnBranchLabel(result.branch) ?? relativePath,
                conflicts: result.conflictCount,
              },
            ),
            result.conflictCount > 0 ? { variant: "warning" } : undefined,
          );
          refreshRemote();
        } else if (result.issue !== "cancelled") {
          toast(
            intl.formatMessage(
              { id: getSvnIssueMessageId(result.issue) },
              { message: result.message },
            ),
            { variant: "warning" },
          );
        }
      } finally {
        setSwitching(false);
        refresh();
      }
    },
    [
      confirm,
      intl,
      refresh,
      refreshRemote,
      request,
      runWithCredentialRetry,
      state.changes,
      svnService,
    ],
  );

  if (!info?.isWorkingCopy || info.availability.status !== "available") {
    return null;
  }

  const label = getSvnBranchLabel(info.branch) ?? intl.formatMessage({ id: "svn.pane.title" });
  const current = info.branch;
  const canSwitch = Boolean(info.canSwitch);
  const renderBranch = (branch: SvnBranchRef) => {
    const isCurrent = current?.relativePath === branch.relativePath;
    return (
      <button
        key={branch.relativePath}
        type="button"
        disabled={switching || isCurrent}
        className="flex h-8 w-full min-w-0 items-center gap-2 rounded-md px-2 text-left text-ui-base text-foreground hover:bg-menu-hover disabled:hover:bg-transparent"
        onClick={() => void switchTo(branch.relativePath)}
      >
        <GitBranchIcon className="size-4 shrink-0 text-foreground-subtle" />
        <span className="min-w-0 flex-1 truncate">{branch.name}</span>
        {isCurrent ? <CheckIcon className="size-4 shrink-0 text-foreground-subtle" /> : null}
      </button>
    );
  };

  return (
    <div className={cn("flex items-center px-1 pt-2", className)}>
      <Popover
        open={open}
        onOpenChange={(nextOpen) => {
          setOpen(nextOpen);
          if (nextOpen && canSwitch && !branches) {
            void loadBranches();
          }
        }}
      >
        <PopoverTrigger asChild>
          <Button
            type="button"
            variant="ghost"
            data-testid={TID_SVN_BRANCH_SWITCHER}
            disabled={switching}
            aria-label={intl.formatMessage({ id: "svn.branch.ariaLabel" })}
            className={cn(
              "min-w-0 max-w-full rounded-full pl-3 pr-2 text-ui-base/relaxed",
              triggerClassName,
            )}
          >
            {current?.kind === "tag" ? (
              <TagIcon className="size-4 text-foreground-subtle" />
            ) : (
              <GitBranchIcon className="size-4 text-foreground-subtle" />
            )}
            <span className="min-w-0 max-w-25 truncate text-left">{label}</span>
            {switching || loading ? (
              <LoaderIcon className="size-3.5 animate-spin text-foreground-subtle" />
            ) : (
              <ChevronDownIcon className="size-3.5 text-foreground-subtle" />
            )}
          </Button>
        </PopoverTrigger>
        <PopoverContent align="start" side={popoverSide} className="w-72 gap-0 bg-menu p-1">
          {current?.kind === "tag" ? (
            <p className="px-2 py-1.5 text-ui-sm text-warning">
              {intl.formatMessage({ id: "svn.branch.onTagHint" })}
            </p>
          ) : null}
          {!canSwitch ? (
            <p className="px-2 py-1.5 text-ui-sm text-foreground-subtle">
              {intl.formatMessage({ id: "svn.branch.cannotSwitch" })}
            </p>
          ) : (
            <>
              {error ? <p className="px-2 py-1.5 text-ui-sm text-destructive">{error}</p> : null}
              {loading ? (
                <p className="px-2 py-1.5 text-ui-sm text-foreground-subtle">
                  {intl.formatMessage({ id: "common.loading" })}
                </p>
              ) : null}
              {branches?.layout === "standard" ? (
                <div className="flex max-h-56 flex-col gap-0.5 overflow-auto">
                  {branches.branches.map(renderBranch)}
                </div>
              ) : null}
              {branches?.layout === "non-standard" || error ? (
                <form
                  className="flex items-center gap-1 p-1"
                  onSubmit={(event: FormEvent<HTMLFormElement>) => {
                    event.preventDefault();
                    if (customPath.trim()) {
                      void switchTo(customPath.trim());
                    }
                  }}
                >
                  <Input
                    value={customPath}
                    className="h-7"
                    placeholder={intl.formatMessage({ id: "svn.branch.customPathPlaceholder" })}
                    onChange={(event) => setCustomPath(event.target.value)}
                  />
                  <Button type="submit" size="sm" variant="outline" disabled={!customPath.trim()}>
                    {intl.formatMessage({ id: "svn.action.switch" })}
                  </Button>
                </form>
              ) : null}
            </>
          )}
        </PopoverContent>
      </Popover>
      {credentialDialog}
    </div>
  );
}
