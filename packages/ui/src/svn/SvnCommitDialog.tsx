import { useEffect, useMemo, useState } from "react";
import type { SvnCredentials, SvnResult } from "@escode/shared";
import {
  TID_SVN_COMMIT_DIALOG,
  TID_SVN_COMMIT_MESSAGE_INPUT,
  TID_SVN_COMMIT_SUBMIT,
} from "@escode/shared";
import { LoaderIcon, SparklesIcon } from "lucide-react";
import { Button } from "@/components/ui/button.js";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog.js";
import { Textarea } from "@/components/ui/textarea.js";
import { toast } from "@/components/ui/toast.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import { logger } from "@/logger.js";
import type { SvnWorkspaceController } from "@/svn/useSvnWorkspace.js";
import { getSvnBranchLabel, getSvnIssueMessageId, summarizeSvnChanges } from "@/svn/svnDisplay.js";

/**
 * SVN 提交弹窗：提交直接写入服务器，没有“提交并推送”（spec：docs/svn/ui.md §4）。
 * 信息为空时先生成再提交，与 Git 提交弹窗一致；过期时保留输入并引导先更新。
 */
export function SvnCommitDialog({
  open,
  controller,
  selectedPaths,
  runWithCredentialRetry,
  onOpenChange,
  onRequestUpdate,
}: {
  open: boolean;
  controller: SvnWorkspaceController;
  selectedPaths: string[];
  runWithCredentialRetry: <T extends object>(
    operation: (credentials?: SvnCredentials) => Promise<SvnResult<T>>,
  ) => Promise<SvnResult<T>>;
  onOpenChange: (open: boolean) => void;
  onRequestUpdate: () => void;
}) {
  const { intl, locale } = useESCodeIntl();
  const { svnService, request, state, refresh, refreshRemote } = controller;
  const [message, setMessage] = useState("");
  const [error, setError] = useState<{ text: string; outOfDate: boolean } | null>(null);
  const [phase, setPhase] = useState<"idle" | "generating" | "committing">("idle");

  useEffect(() => {
    if (open) {
      setError(null);
      setPhase("idle");
    }
  }, [open]);

  const selectedChanges = useMemo(() => {
    const selected = new Set(selectedPaths);
    return state.changes.filter((change) => selected.has(change.path));
  }, [selectedPaths, state.changes]);
  const summary = summarizeSvnChanges(selectedChanges);
  const unversionedCount = selectedChanges.filter(
    (change) => change.status === "unversioned",
  ).length;
  const branchLabel = getSvnBranchLabel(state.info?.branch);

  const generate = async (): Promise<string | null> => {
    if (!svnService) {
      return null;
    }
    setPhase("generating");
    try {
      const result = await svnService.generateCommitMessage({
        ...request,
        paths: selectedPaths,
        locale,
      });
      if (!result.ok) {
        setError({
          text: intl.formatMessage(
            { id: getSvnIssueMessageId(result.issue) },
            { message: result.message },
          ),
          outOfDate: false,
        });
        return null;
      }
      setMessage(result.message);
      return result.message;
    } catch (generateError) {
      const text = generateError instanceof Error ? generateError.message : String(generateError);
      logger.warn("[SvnCommitDialog] 生成提交信息失败", { error: text });
      setError({
        text: intl.formatMessage({ id: "svn.commit.generateFailed" }, { message: text }),
        outOfDate: false,
      });
      return null;
    } finally {
      setPhase("idle");
    }
  };

  const commit = async () => {
    if (!svnService) {
      return;
    }
    setError(null);
    const finalMessage = message.trim() ? message : await generate();
    if (!finalMessage?.trim()) {
      return;
    }
    setPhase("committing");
    try {
      const result = await runWithCredentialRetry((credentials) =>
        svnService.commit({
          ...request,
          paths: selectedPaths,
          message: finalMessage,
          ...(credentials ? { credentials } : {}),
        }),
      );
      if (result.ok) {
        toast(intl.formatMessage({ id: "svn.toast.committed" }, { revision: result.revision }));
        setMessage("");
        onOpenChange(false);
        refreshRemote();
        return;
      }
      if (result.issue === "cancelled") {
        return;
      }
      setError({
        text: intl.formatMessage(
          { id: getSvnIssueMessageId(result.issue) },
          { message: result.message },
        ),
        outOfDate: result.issue === "out-of-date",
      });
    } catch (commitError) {
      setError({
        text: commitError instanceof Error ? commitError.message : String(commitError),
        outOfDate: false,
      });
    } finally {
      setPhase("idle");
      refresh();
    }
  };

  const busy = phase !== "idle";

  return (
    <Dialog open={open} onOpenChange={(next) => !busy && onOpenChange(next)}>
      <DialogContent
        data-testid={TID_SVN_COMMIT_DIALOG}
        className="max-w-lg gap-0 overflow-hidden rounded-2xl p-0"
      >
        <DialogHeader className="gap-2 px-6 py-5 pb-0">
          <DialogTitle className="text-lg font-medium text-foreground">
            {intl.formatMessage({ id: "svn.commit.title" })}
          </DialogTitle>
          <DialogDescription className="text-ui-base text-foreground-subtle">
            {branchLabel ? `${branchLabel} · ` : ""}
            {intl.formatMessage({ id: "svn.commit.fileCount" }, { count: summary.fileCount })}{" "}
            <span className="text-diff-added">+{summary.added}</span>{" "}
            <span className="text-diff-removed">-{summary.removed}</span>
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-3 px-6 py-5">
          <div className="relative">
            <Textarea
              data-testid={TID_SVN_COMMIT_MESSAGE_INPUT}
              value={message}
              rows={5}
              disabled={busy}
              placeholder={intl.formatMessage({ id: "svn.commit.messagePlaceholder" })}
              onChange={(event) => setMessage(event.target.value)}
            />
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              className="absolute right-2 bottom-2"
              aria-label={intl.formatMessage({ id: "svn.commit.generate" })}
              disabled={busy || !svnService}
              onClick={() => void generate()}
            >
              {phase === "generating" ? <LoaderIcon className="animate-spin" /> : <SparklesIcon />}
            </Button>
          </div>
          {unversionedCount > 0 ? (
            <p className="text-ui-sm text-foreground-subtle">
              {intl.formatMessage({ id: "svn.commit.willAdd" }, { count: unversionedCount })}
            </p>
          ) : null}
          {error ? (
            <div className="flex items-start gap-2 rounded-lg bg-destructive/10 px-3 py-2">
              <p className="min-w-0 flex-1 whitespace-pre-line text-ui-base text-destructive">
                {error.text}
              </p>
              {error.outOfDate ? (
                <Button type="button" variant="outline" size="sm" onClick={onRequestUpdate}>
                  {intl.formatMessage({ id: "svn.action.update" })}
                </Button>
              ) : null}
            </div>
          ) : null}
        </div>
        <DialogFooter className="gap-2 px-6 pb-5">
          <Button
            type="button"
            variant="secondary"
            size="lg"
            disabled={busy}
            onClick={() => onOpenChange(false)}
          >
            {intl.formatMessage({ id: "common.cancel" })}
          </Button>
          <Button
            type="button"
            data-testid={TID_SVN_COMMIT_SUBMIT}
            size="lg"
            disabled={busy || selectedPaths.length === 0 || summary.conflictCount > 0}
            onClick={() => void commit()}
          >
            {phase === "committing" ? <LoaderIcon className="animate-spin" /> : null}
            {intl.formatMessage({ id: "svn.commit.submit" })}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
