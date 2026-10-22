import { useEffect, useState } from "react";
import type { SvnCredentials, SvnExternalDefinition, SvnResult } from "@escode/shared";
import { TID_SVN_EXTERNALS_DIALOG } from "@escode/shared";
import { LinkIcon } from "lucide-react";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import type { SvnWorkspaceController } from "@/svn/useSvnWorkspace.js";
import { getSvnIssueMessageId } from "@/svn/svnDisplay.js";

/** 只读外部定义列表（spec：docs/svn/ui.md §14）。 */
export function SvnExternalsDialog({
  open,
  controller,
  runWithCredentialRetry,
  onOpenChange,
}: {
  open: boolean;
  controller: SvnWorkspaceController;
  runWithCredentialRetry: <T extends object>(
    operation: (credentials?: SvnCredentials) => Promise<SvnResult<T>>,
  ) => Promise<SvnResult<T>>;
  onOpenChange: (open: boolean) => void;
}) {
  const { intl } = useESCodeIntl();
  const { svnService, request } = controller;
  const [externals, setExternals] = useState<SvnExternalDefinition[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open || !svnService) {
      return;
    }
    let cancelled = false;
    setLoading(true);
    setError(null);
    setExternals([]);
    void (async () => {
      try {
        const result = await runWithCredentialRetry((credentials) =>
          svnService.getExternals({
            ...request,
            ...(credentials ? { credentials } : {}),
          }),
        );
        if (cancelled) {
          return;
        }
        if (result.ok) {
          setExternals(result.externals);
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
  }, [intl, open, request, runWithCredentialRetry, svnService]);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        data-testid={TID_SVN_EXTERNALS_DIALOG}
        className="flex max-h-[min(78vh,720px)] max-w-3xl flex-col gap-0 overflow-hidden rounded-2xl p-0"
      >
        <DialogHeader className="border-b border-border px-5 py-4">
          <DialogTitle className="text-ui-lg font-medium text-foreground">
            {intl.formatMessage({ id: "svn.externals.title" })}
          </DialogTitle>
          <p className="text-ui-sm text-foreground-subtle">
            {intl.formatMessage({ id: "svn.externals.readonly" })}
          </p>
        </DialogHeader>
        <div className="min-h-0 flex-1 overflow-auto p-2">
          {loading ? (
            <p className="px-3 py-3 text-ui-base text-foreground-subtle">
              {intl.formatMessage({ id: "common.loading" })}
            </p>
          ) : error ? (
            <p className="whitespace-pre-line px-3 py-3 text-ui-base text-destructive">{error}</p>
          ) : externals.length === 0 ? (
            <p className="px-3 py-3 text-ui-base text-foreground-subtle">
              {intl.formatMessage({ id: "svn.externals.empty" })}
            </p>
          ) : (
            externals.map((external) => (
              <div
                key={`${external.path}:${external.name}`}
                data-svn-external={external.name}
                className="flex items-start gap-2 rounded-lg px-3 py-2 hover:bg-surface-hover"
              >
                <LinkIcon className="mt-0.5 size-4 shrink-0 text-foreground-subtle" />
                <div className="min-w-0 flex-1">
                  <p className="truncate text-ui-base text-foreground">
                    {external.path === "." ? external.name : `${external.path}/${external.name}`}
                  </p>
                  <p className="truncate font-mono text-ui-sm text-foreground-subtle">
                    {external.url}
                    {external.pegRevision !== undefined
                      ? `@${external.pegRevision}`
                      : external.revision !== undefined
                        ? ` · ${intl.formatMessage(
                            { id: "svn.externals.revision" },
                            { revision: external.revision },
                          )}`
                        : ""}
                  </p>
                </div>
              </div>
            ))
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
