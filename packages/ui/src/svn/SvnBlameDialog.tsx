import { useEffect, useState } from "react";
import type { SvnBlameLine, SvnCredentials, SvnResult } from "@escode/shared";
import { TID_SVN_BLAME_DIALOG } from "@escode/shared";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import type { SvnWorkspaceController } from "@/svn/useSvnWorkspace.js";
import { getSvnIssueMessageId } from "@/svn/svnDisplay.js";

/**
 * 逐行 Blame（spec：docs/svn/ui.md §12）。
 * 只读；行内容来自与 blame 相同修订的 cat 输出，服务端最多返回 5000 行。
 */
export function SvnBlameDialog({
  open,
  path,
  controller,
  runWithCredentialRetry,
  onOpenChange,
}: {
  open: boolean;
  path: string;
  controller: SvnWorkspaceController;
  runWithCredentialRetry: <T extends object>(
    operation: (credentials?: SvnCredentials) => Promise<SvnResult<T>>,
  ) => Promise<SvnResult<T>>;
  onOpenChange: (open: boolean) => void;
}) {
  const { intl } = useESCodeIntl();
  const { svnService, request } = controller;
  const [lines, setLines] = useState<SvnBlameLine[]>([]);
  const [truncated, setTruncated] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open || !path || !svnService) {
      return;
    }
    let cancelled = false;
    setLoading(true);
    setError(null);
    setLines([]);
    setTruncated(false);
    void (async () => {
      try {
        const result = await runWithCredentialRetry((credentials) =>
          svnService.getBlame({
            ...request,
            path,
            ...(credentials ? { credentials } : {}),
          }),
        );
        if (cancelled) {
          return;
        }
        if (result.ok) {
          setLines(result.lines);
          setTruncated(result.truncated);
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
          setLoading(false);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [intl, open, path, request, runWithCredentialRetry, svnService]);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        data-testid={TID_SVN_BLAME_DIALOG}
        className="flex h-[min(78vh,720px)] max-w-4xl flex-col gap-0 overflow-hidden rounded-2xl p-0"
      >
        <DialogHeader className="border-b border-border px-5 py-4">
          <DialogTitle className="truncate text-ui-lg font-medium text-foreground">
            {intl.formatMessage({ id: "svn.blame.title" }, { path })}
          </DialogTitle>
        </DialogHeader>
        <div className="min-h-0 flex-1 overflow-auto font-mono text-ui-sm">
          {loading ? (
            <p className="px-4 py-3 text-foreground-subtle">
              {intl.formatMessage({ id: "common.loading" })}
            </p>
          ) : error ? (
            <p className="whitespace-pre-line px-4 py-3 text-destructive">{error}</p>
          ) : lines.length === 0 ? (
            <p className="px-4 py-3 text-foreground-subtle">
              {intl.formatMessage({ id: "svn.blame.empty" })}
            </p>
          ) : (
            lines.map((line) => (
              <div
                key={`${line.lineNumber}:${line.revision}`}
                data-svn-blame-line={line.lineNumber}
                className="grid grid-cols-[3.5rem_4rem_8rem_minmax(0,1fr)] items-start gap-2 px-3 py-0.5 hover:bg-surface-hover"
              >
                <span className="text-right text-foreground-subtlest">{line.lineNumber}</span>
                <span className="text-foreground-subtle">r{line.revision}</span>
                <span className="truncate text-foreground-subtle" title={line.author}>
                  {line.author}
                </span>
                <pre className="whitespace-pre-wrap break-all text-foreground">{line.content}</pre>
              </div>
            ))
          )}
          {truncated ? (
            <p className="border-t border-border px-4 py-2 text-foreground-subtle">
              {intl.formatMessage({ id: "svn.blame.truncated" }, { count: lines.length })}
            </p>
          ) : null}
        </div>
      </DialogContent>
    </Dialog>
  );
}
