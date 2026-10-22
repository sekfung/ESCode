import { TID_SVN_STATUS_CHANGES, TID_SVN_STATUS_COMMIT } from "@escode/shared";
import { useMemo, useState } from "react";
import { FileDiffIcon, GitCommitHorizontalIcon } from "lucide-react";
import { cn } from "@/components/lib/utils.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import { SvnBranchSwitcher } from "@/svn/SvnBranchSwitcher.js";
import { SvnCommitDialog } from "@/svn/SvnCommitDialog.js";
import { useSvnCredentialPrompt } from "@/svn/useSvnCredentialPrompt.js";
import type { SvnWorkspaceController } from "@/svn/useSvnWorkspace.js";
import {
  getSelectedSvnPaths,
  getSvnCommitBlockReason,
  reconcileSvnSelection,
  summarizeSvnChanges,
} from "@/svn/svnDisplay.js";

const ROW_CLASS =
  "flex h-8 w-full min-w-0 items-center gap-2 rounded-lg px-2 text-left text-ui-base text-[var(--color-foreground)] transition-colors";
const ROW_ENABLED_CLASS =
  "hover:bg-[var(--color-hover)] hover:text-[var(--color-foreground)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--color-input-border-focused)]";

/** 会话状态面板中 SVN workspace 的“环境”区内容（对应 Git 的改动 / 分支 / 提交三行）。 */
export function SvnStatusSectionContent({
  controller,
  onOpenReview,
}: {
  controller: SvnWorkspaceController;
  onOpenReview?: () => void;
}) {
  const { intl } = useESCodeIntl();
  const { state } = controller;
  const summary = summarizeSvnChanges(state.changes);
  const { runWithCredentialRetry, credentialDialog } = useSvnCredentialPrompt(state.info?.url);
  const [commitOpen, setCommitOpen] = useState(false);
  // 状态区没有逐文件勾选，提交范围取面板的默认勾选（已纳入版本控制的可提交改动）。
  const defaultPaths = useMemo(
    () => getSelectedSvnPaths(reconcileSvnSelection(new Map(), state.changes)),
    [state.changes],
  );
  const commitBlockReason = getSvnCommitBlockReason(state.changes, defaultPaths);
  const available =
    state.info?.availability.status === "available" && state.info.isWorkingCopy === true;

  return (
    <div className="space-y-0">
      <button
        type="button"
        data-testid={TID_SVN_STATUS_CHANGES}
        disabled={!onOpenReview}
        className={cn(ROW_CLASS, onOpenReview ? ROW_ENABLED_CLASS : "cursor-default opacity-50")}
        onClick={() => onOpenReview?.()}
      >
        <FileDiffIcon className="size-4 shrink-0 text-[var(--color-foreground)]" />
        <span className="min-w-0 flex-1 truncate">
          {intl.formatMessage({ id: "chat.statusPanel.changes" })}
        </span>
        {summary.conflictCount > 0 ? (
          <span className="shrink-0 text-ui-sm text-destructive">
            {intl.formatMessage(
              { id: "svn.status.conflictCount" },
              { count: summary.conflictCount },
            )}
          </span>
        ) : null}
        <span className="shrink-0 font-mono tabular-nums">
          <span className="text-[var(--color-diff-added)]">+{summary.added}</span>{" "}
          <span className="text-[var(--color-diff-removed)]">-{summary.removed}</span>
        </span>
      </button>
      <SvnBranchSwitcher
        controller={controller}
        className="w-full px-0 pt-0"
        triggerClassName="flex h-8 w-full min-w-0 justify-start gap-2 rounded-lg px-2 text-left text-ui-base text-[var(--color-foreground)] hover:bg-[var(--color-hover)] hover:text-[var(--color-foreground)] [&>span]:max-w-[calc(100%-3.5rem)]"
        popoverSide="left"
      />
      {available ? (
        <button
          type="button"
          data-testid={TID_SVN_STATUS_COMMIT}
          disabled={commitBlockReason !== null}
          title={
            commitBlockReason
              ? intl.formatMessage({ id: `svn.commit.blocked.${commitBlockReason}` })
              : undefined
          }
          className={cn(
            ROW_CLASS,
            commitBlockReason === null ? ROW_ENABLED_CLASS : "cursor-default opacity-50",
          )}
          onClick={() => setCommitOpen(true)}
        >
          <GitCommitHorizontalIcon className="size-4 shrink-0" />
          <span className="min-w-0 flex-1 truncate">
            {intl.formatMessage({ id: "svn.action.commit" })}
          </span>
        </button>
      ) : null}
      <SvnCommitDialog
        open={commitOpen}
        controller={controller}
        selectedPaths={defaultPaths}
        runWithCredentialRetry={runWithCredentialRetry}
        onOpenChange={setCommitOpen}
        onRequestUpdate={() => {
          setCommitOpen(false);
          onOpenReview?.();
        }}
      />
      {credentialDialog}
    </div>
  );
}
