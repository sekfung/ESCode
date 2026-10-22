import { useCallback, useEffect, useMemo, useState } from "react";
import type {
  SvnBranchListResult,
  SvnCredentials,
  SvnMergeResult,
  SvnResult,
} from "@escode/shared";
import {
  TID_SVN_MERGE_DIALOG,
  TID_SVN_MERGE_END,
  TID_SVN_MERGE_RANGE,
  TID_SVN_MERGE_SOURCE,
  TID_SVN_MERGE_START,
  TID_SVN_MERGE_SUBMIT,
} from "@escode/shared";
import { GitMergeIcon, LoaderIcon } from "lucide-react";
import { Button } from "@/components/ui/button.js";
import { Checkbox } from "@/components/ui/checkbox.js";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog.js";
import { Input } from "@/components/ui/input.js";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select.js";
import { toast } from "@/components/ui/toast.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import { logger } from "@/logger.js";
import type { SvnWorkspaceController } from "@/svn/useSvnWorkspace.js";
import {
  getSvnBranchLabel,
  getSvnIssueMessageId,
  getSvnMergeSourceOptions,
  parseSvnMergeRange,
  validateSvnMergeRange,
} from "@/svn/svnDisplay.js";

/**
 * SVN 合并弹窗（spec：docs/svn/ui.md §11）。
 * 合并只修改工作副本：冲突进入现有冲突分组，提交仍由用户显式发起。
 */
export function SvnMergeDialog({
  open,
  controller,
  runWithCredentialRetry,
  onOpenChange,
  onMerged,
}: {
  open: boolean;
  controller: SvnWorkspaceController;
  runWithCredentialRetry: <T extends object>(
    operation: (credentials?: SvnCredentials) => Promise<SvnResult<T>>,
  ) => Promise<SvnResult<T>>;
  onOpenChange: (open: boolean) => void;
  onMerged?: (result: SvnMergeResult) => void;
}) {
  const { intl } = useESCodeIntl();
  const { svnService, request, state, refresh } = controller;
  const [branches, setBranches] = useState<SvnBranchListResult | null>(null);
  const [loadingBranches, setLoadingBranches] = useState(false);
  const [sourcePath, setSourcePath] = useState("");
  const [customPath, setCustomPath] = useState("");
  const [rangeEnabled, setRangeEnabled] = useState(false);
  const [startRevision, setStartRevision] = useState("");
  const [endRevision, setEndRevision] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [merging, setMerging] = useState(false);

  const current = state.info?.branch;
  const currentLabel = getSvnBranchLabel(current);
  const canMerge = state.info?.canSwitch === true;
  const options = useMemo(() => getSvnMergeSourceOptions(branches, current), [branches, current]);
  const relativeSourcePath = branches?.layout === "standard" ? sourcePath : customPath.trim();
  const rangeError = rangeEnabled ? validateSvnMergeRange(startRevision, endRevision) : null;
  const busy = merging || loadingBranches;

  useEffect(() => {
    if (!open) {
      return;
    }
    setError(null);
    setSourcePath("");
    setCustomPath("");
    setRangeEnabled(false);
    setStartRevision("");
    setEndRevision("");
    setBranches(null);
    setLoadingBranches(true);
    if (!svnService) {
      setLoadingBranches(false);
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const result = await runWithCredentialRetry((credentials) =>
          svnService.listBranches({ ...request, ...(credentials ? { credentials } : {}) }),
        );
        if (cancelled) {
          return;
        }
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
        if (!cancelled) {
          setError(loadError instanceof Error ? loadError.message : String(loadError));
        }
      } finally {
        if (!cancelled) {
          setLoadingBranches(false);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [intl, open, request, runWithCredentialRetry, svnService]);

  useEffect(() => {
    if (open && options.length > 0 && !sourcePath) {
      setSourcePath(options[0]!.relativePath);
    }
  }, [open, options, sourcePath]);

  const merge = useCallback(async () => {
    if (!svnService || !relativeSourcePath) {
      return;
    }
    const range = rangeEnabled ? parseSvnMergeRange(startRevision, endRevision) : null;
    if (rangeEnabled && !range) {
      setError(
        intl.formatMessage({
          id: rangeError === "missing" ? "svn.merge.rangeMissing" : "svn.merge.rangeInvalid",
        }),
      );
      return;
    }
    setError(null);
    setMerging(true);
    try {
      const result = await runWithCredentialRetry((credentials) =>
        svnService.merge({
          ...request,
          relativeSourcePath,
          ...(range ?? {}),
          ...(credentials ? { credentials } : {}),
        }),
      );
      if (result.ok) {
        toast(
          intl.formatMessage(
            {
              id: result.conflictCount > 0 ? "svn.merge.successWithConflicts" : "svn.merge.success",
            },
            { count: result.mergedFileCount, conflicts: result.conflictCount },
          ),
          result.conflictCount > 0 ? { variant: "warning" } : undefined,
        );
        onMerged?.(result);
        onOpenChange(false);
      } else if (result.issue !== "cancelled") {
        setError(
          intl.formatMessage(
            { id: getSvnIssueMessageId(result.issue) },
            { message: result.message },
          ),
        );
      }
    } catch (mergeError) {
      const message = mergeError instanceof Error ? mergeError.message : String(mergeError);
      logger.warn("[SvnMergeDialog] SVN 合并失败", {
        workspacePath: request.workspacePath,
        error: message,
      });
      setError(message);
    } finally {
      setMerging(false);
      refresh();
    }
  }, [
    intl,
    onMerged,
    onOpenChange,
    rangeEnabled,
    rangeError,
    refresh,
    relativeSourcePath,
    request,
    runWithCredentialRetry,
    startRevision,
    endRevision,
    svnService,
  ]);

  return (
    <Dialog open={open} onOpenChange={(next) => !merging && onOpenChange(next)}>
      <DialogContent
        data-testid={TID_SVN_MERGE_DIALOG}
        className="max-w-lg gap-0 overflow-hidden rounded-2xl p-0"
      >
        <DialogHeader className="gap-2 px-6 py-5 pb-0">
          <DialogTitle className="flex items-center gap-2 text-lg font-medium text-foreground">
            <GitMergeIcon className="size-4 text-foreground-subtle" />
            {intl.formatMessage({ id: "svn.merge.title" })}
          </DialogTitle>
          <DialogDescription className="text-ui-base text-foreground-subtle">
            {intl.formatMessage({ id: "svn.merge.description" })}
            {currentLabel ? ` · ${currentLabel}` : ""}
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-4 px-6 py-5">
          {!canMerge ? (
            <p className="rounded-lg bg-warning/10 px-3 py-2 text-ui-base text-foreground">
              {intl.formatMessage({ id: "svn.merge.cannotSwitch" })}
            </p>
          ) : null}
          {current?.kind === "tag" ? (
            <p className="text-ui-sm text-foreground-subtle">
              {intl.formatMessage({ id: "svn.merge.tagHint" })}
            </p>
          ) : null}

          <div className="space-y-2">
            <p className="text-ui-sm font-medium text-foreground">
              {intl.formatMessage({ id: "svn.merge.sourceLabel" })}
            </p>
            {branches?.layout === "standard" ? (
              options.length > 0 ? (
                <Select value={sourcePath} onValueChange={setSourcePath} disabled={busy}>
                  <SelectTrigger data-testid={TID_SVN_MERGE_SOURCE} size="lg" className="w-full">
                    <SelectValue
                      placeholder={intl.formatMessage({ id: "svn.merge.sourcePlaceholder" })}
                    />
                  </SelectTrigger>
                  <SelectContent>
                    {options.map((option) => (
                      <SelectItem key={option.relativePath} value={option.relativePath}>
                        {option.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              ) : (
                <p className="text-ui-sm text-foreground-subtle">
                  {intl.formatMessage({ id: "svn.merge.noSource" })}
                </p>
              )
            ) : (
              <Input
                data-testid={TID_SVN_MERGE_SOURCE}
                value={customPath}
                disabled={busy}
                placeholder={intl.formatMessage({ id: "svn.merge.customPathPlaceholder" })}
                onChange={(event) => setCustomPath(event.target.value)}
              />
            )}
          </div>

          <div className="space-y-2">
            <label className="flex items-center gap-2 text-ui-sm text-foreground">
              <Checkbox
                data-testid={TID_SVN_MERGE_RANGE}
                checked={rangeEnabled}
                disabled={busy}
                onCheckedChange={(checked) => setRangeEnabled(checked === true)}
              />
              {intl.formatMessage({ id: "svn.merge.rangeLabel" })}
            </label>
            {rangeEnabled ? (
              <div className="flex items-center gap-2">
                <Input
                  data-testid={TID_SVN_MERGE_START}
                  value={startRevision}
                  inputMode="numeric"
                  disabled={busy}
                  placeholder={intl.formatMessage({ id: "svn.merge.startPlaceholder" })}
                  onChange={(event) => setStartRevision(event.target.value)}
                />
                <span className="text-foreground-subtle">:</span>
                <Input
                  data-testid={TID_SVN_MERGE_END}
                  value={endRevision}
                  inputMode="numeric"
                  disabled={busy}
                  placeholder={intl.formatMessage({ id: "svn.merge.endPlaceholder" })}
                  onChange={(event) => setEndRevision(event.target.value)}
                />
              </div>
            ) : null}
            {rangeError ? (
              <p className="text-ui-sm text-destructive">
                {intl.formatMessage({
                  id:
                    rangeError === "missing" ? "svn.merge.rangeMissing" : "svn.merge.rangeInvalid",
                })}
              </p>
            ) : null}
          </div>

          {state.changes.length > 0 ? (
            <p className="text-ui-sm text-foreground-subtle">
              {intl.formatMessage({ id: "svn.merge.dirtyHint" })}
            </p>
          ) : null}
          {error ? (
            <p className="whitespace-pre-line rounded-lg bg-destructive/10 px-3 py-2 text-ui-base text-destructive">
              {error}
            </p>
          ) : null}
        </div>
        <DialogFooter className="gap-2 px-6 pb-5">
          <Button
            type="button"
            variant="secondary"
            size="lg"
            disabled={merging}
            onClick={() => onOpenChange(false)}
          >
            {intl.formatMessage({ id: "common.cancel" })}
          </Button>
          <Button
            type="button"
            data-testid={TID_SVN_MERGE_SUBMIT}
            size="lg"
            disabled={
              busy || !canMerge || !relativeSourcePath || (rangeEnabled && rangeError !== null)
            }
            onClick={() => void merge()}
          >
            {merging ? <LoaderIcon className="animate-spin" /> : null}
            {intl.formatMessage({ id: "svn.merge.submit" })}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
