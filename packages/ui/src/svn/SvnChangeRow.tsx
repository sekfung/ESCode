import { memo } from "react";
import type { GitDiffResult, SvnFileChange, SvnResolveAccept } from "@escode/shared";
import {
  TID_SVN_BLAME_BUTTON,
  TID_SVN_FILE_HISTORY_BUTTON,
  TID_SVN_LOCK_BUTTON,
} from "@escode/shared";
import { FingerprintIcon, HistoryIcon, LockIcon, LockOpenIcon } from "lucide-react";
import { Button } from "@/components/ui/button.js";
import { Checkbox } from "@/components/ui/checkbox.js";
import { cn } from "@/components/lib/utils.js";
import { GitPaneChangeCard } from "@/GitPaneChangeCard.js";
import type { GitPaneFileChange } from "@/hooks/useGitRepository.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import type { CodePreviewSettings } from "@/store/index.js";
import type { ResolvedTheme } from "@/useTheme.js";
import {
  canShowSvnBlame,
  canShowSvnFileHistory,
  canShowSvnLock,
  getSvnStatusLetter,
  isSvnChangeCommittable,
} from "@/svn/svnDisplay.js";

export interface SvnChangeRowProps {
  change: SvnFileChange;
  cardModel: GitPaneFileChange;
  checked: boolean;
  disabled: boolean;
  diffState: GitDiffResult | null;
  isDiffLoading: boolean;
  isExpanded: boolean;
  codePreviewSettings: CodePreviewSettings;
  resolvedTheme: ResolvedTheme;
  contextMenuLabels: {
    copyAbsolutePath: string;
    copyRelativePath: string;
    revealInFileManager: string;
    revealInFileTree: string;
  };
  canRevealInFileManager: boolean;
  onCheckedChange: (path: string, checked: boolean) => void;
  onOpenChange: (change: GitPaneFileChange, nextOpen: boolean) => void;
  onResolve: (path: string, accept: SvnResolveAccept) => void;
  onOpenFile: (change: GitPaneFileChange) => void;
  onCopyAbsolutePath: (change: GitPaneFileChange) => void;
  onCopyRelativePath: (change: GitPaneFileChange) => void;
  onRevealInFileManager: (change: GitPaneFileChange) => void;
  onRevealInFileTree?: (change: GitPaneFileChange) => void;
  onShowFileHistory: (change: SvnFileChange) => void;
  onShowBlame: (change: SvnFileChange) => void;
  onLock: (change: SvnFileChange) => void;
  onUnlock: (change: SvnFileChange) => void;
}

const STATUS_TONE: Partial<Record<SvnFileChange["status"], string>> = {
  added: "text-diff-added",
  unversioned: "text-diff-added",
  deleted: "text-diff-removed",
  missing: "text-diff-removed",
  conflicted: "text-destructive",
};

/** SVN 变更行：勾选框 + 状态字母 + 复用 Git 面板的变更卡片（diff 展示一致）。 */
export const SvnChangeRow = memo(function SvnChangeRow({
  change,
  cardModel,
  checked,
  disabled,
  diffState,
  isDiffLoading,
  isExpanded,
  codePreviewSettings,
  resolvedTheme,
  contextMenuLabels,
  canRevealInFileManager,
  onCheckedChange,
  onOpenChange,
  onResolve,
  onOpenFile,
  onCopyAbsolutePath,
  onCopyRelativePath,
  onRevealInFileManager,
  onRevealInFileTree,
  onShowFileHistory,
  onShowBlame,
  onLock,
  onUnlock,
}: SvnChangeRowProps) {
  const { intl } = useESCodeIntl();
  const committable = isSvnChangeCommittable(change);
  const conflictKinds = change.conflict?.kinds ?? [];
  const isTextConflict =
    change.status === "conflicted" && conflictKinds.length === 1 && conflictKinds[0] === "text";

  return (
    <div className="w-full min-w-0" data-svn-change-path={change.path}>
      <div className="flex w-full min-w-0 items-start">
        <div className="flex h-8 shrink-0 items-center gap-2 pl-3">
          <Checkbox
            aria-label={intl.formatMessage({ id: "svn.pane.selectFile" }, { path: change.path })}
            checked={committable && checked}
            disabled={!committable || disabled}
            onCheckedChange={(next) => onCheckedChange(change.path, next === true)}
          />
          <span
            className={cn(
              "w-3 text-center font-mono text-ui-sm text-foreground-subtle",
              STATUS_TONE[change.status],
            )}
            title={intl.formatMessage({ id: `svn.status.${change.status}` })}
          >
            {getSvnStatusLetter(change.status)}
          </span>
        </div>
        <div className="min-w-0 flex-1">
          <GitPaneChangeCard
            change={cardModel}
            contextMenuLabels={contextMenuLabels}
            diffState={diffState}
            isDiffLoading={isDiffLoading}
            isExpanded={isExpanded}
            canRevealInFileManager={canRevealInFileManager}
            codePreviewSettings={codePreviewSettings}
            resolvedTheme={resolvedTheme}
            onCopyAbsolutePath={onCopyAbsolutePath}
            onCopyRelativePath={onCopyRelativePath}
            onOpenChange={onOpenChange}
            onRevealInFileManager={onRevealInFileManager}
            onRevealInFileTree={onRevealInFileTree}
          />
        </div>
        <div className="flex h-8 shrink-0 items-center gap-0.5 pr-2">
          {change.lock ? (
            <Button
              type="button"
              data-testid={TID_SVN_LOCK_BUTTON}
              variant="ghost"
              size="icon-sm"
              aria-label={intl.formatMessage({ id: "svn.lock.unlock" })}
              title={intl.formatMessage({ id: "svn.lock.owner" }, { owner: change.lock.owner })}
              disabled={disabled}
              onClick={() => onUnlock(change)}
            >
              <LockIcon className="size-3.5 text-warning" />
            </Button>
          ) : canShowSvnLock(change) ? (
            <Button
              type="button"
              data-testid={TID_SVN_LOCK_BUTTON}
              variant="ghost"
              size="icon-sm"
              aria-label={intl.formatMessage({ id: "svn.lock.lock" })}
              title={intl.formatMessage({ id: "svn.lock.lock" })}
              disabled={disabled}
              onClick={() => onLock(change)}
            >
              <LockOpenIcon className="size-3.5" />
            </Button>
          ) : null}
          {canShowSvnFileHistory(change) ? (
            <Button
              type="button"
              data-testid={TID_SVN_FILE_HISTORY_BUTTON}
              variant="ghost"
              size="icon-sm"
              aria-label={intl.formatMessage({ id: "svn.row.fileHistory" })}
              title={intl.formatMessage({ id: "svn.row.fileHistory" })}
              disabled={disabled}
              onClick={() => onShowFileHistory(change)}
            >
              <HistoryIcon className="size-3.5" />
            </Button>
          ) : null}
          {canShowSvnBlame(change) ? (
            <Button
              type="button"
              data-testid={TID_SVN_BLAME_BUTTON}
              variant="ghost"
              size="icon-sm"
              aria-label={intl.formatMessage({ id: "svn.row.blame" })}
              title={intl.formatMessage({ id: "svn.row.blame" })}
              disabled={disabled}
              onClick={() => onShowBlame(change)}
            >
              <FingerprintIcon className="size-3.5" />
            </Button>
          ) : null}
        </div>
      </div>
      {change.lock ? (
        <p className="pb-2 pl-12 pr-3 text-ui-sm text-foreground-subtle">
          <LockIcon className="mr-1 inline size-3" />
          {intl.formatMessage({ id: "svn.lock.owner" }, { owner: change.lock.owner })}
          {change.lock.comment ? ` · ${change.lock.comment}` : ""}
        </p>
      ) : null}
      {change.status === "conflicted" ? (
        <div className="flex flex-wrap items-center gap-1 pb-2 pl-12 pr-3">
          {isTextConflict ? (
            <>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                disabled={disabled}
                onClick={() => onResolve(change.path, "mine-full")}
              >
                {intl.formatMessage({ id: "svn.conflict.useMine" })}
              </Button>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                disabled={disabled}
                onClick={() => onResolve(change.path, "theirs-full")}
              >
                {intl.formatMessage({ id: "svn.conflict.useTheirs" })}
              </Button>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                disabled={disabled}
                onClick={() => onOpenFile(cardModel)}
              >
                {intl.formatMessage({ id: "svn.conflict.openFile" })}
              </Button>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                disabled={disabled}
                onClick={() => onResolve(change.path, "working")}
              >
                {intl.formatMessage({ id: "svn.conflict.markResolved" })}
              </Button>
            </>
          ) : (
            <p className="text-ui-sm text-foreground-subtle">
              {intl.formatMessage(
                {
                  id: conflictKinds.includes("tree")
                    ? "svn.conflict.treeReadonly"
                    : "svn.conflict.propertyReadonly",
                },
                { description: change.conflict?.description ?? "" },
              )}
            </p>
          )}
        </div>
      ) : null}
      {change.status === "missing" ? (
        <p className="pb-2 pl-12 pr-3 text-ui-sm text-foreground-subtle">
          {intl.formatMessage({ id: "svn.pane.missingHint" })}
        </p>
      ) : null}
    </div>
  );
});
