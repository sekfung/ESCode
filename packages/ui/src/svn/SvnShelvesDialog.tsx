import { useCallback, useEffect, useMemo, useState } from "react";
import type { SvnCredentials, SvnResult, SvnShelf } from "@escode/shared";
import { TID_SVN_SHELF_NAME, TID_SVN_SHELVES_DIALOG, TID_SVN_SHELVE_SUBMIT } from "@escode/shared";
import { LoaderIcon } from "lucide-react";
import { Button } from "@/components/ui/button.js";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog.js";
import { Input } from "@/components/ui/input.js";
import { toast } from "@/components/ui/toast.js";
import { useConfirmDialog } from "@/hooks/useConfirmDialog.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import type { SvnWorkspaceController } from "@/svn/useSvnWorkspace.js";
import { getSvnIssueMessageId } from "@/svn/svnDisplay.js";

/** 可移植暂存架（spec：docs/svn/ui.md §16）。 */
export function SvnShelvesDialog({
  open,
  controller,
  selectedPaths,
  runWithCredentialRetry,
  onOpenChange,
}: {
  open: boolean;
  controller: SvnWorkspaceController;
  selectedPaths: string[];
  runWithCredentialRetry: <T extends object>(
    operation: (credentials?: SvnCredentials) => Promise<SvnResult<T>>,
  ) => Promise<SvnResult<T>>;
  onOpenChange: (open: boolean) => void;
}) {
  const { intl, locale } = useESCodeIntl();
  const { svnService, request, refresh } = controller;
  const confirm = useConfirmDialog();
  const [shelves, setShelves] = useState<SvnShelf[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);
  const dateFormatter = useMemo(
    () => new Intl.DateTimeFormat(locale, { dateStyle: "short", timeStyle: "short" }),
    [locale],
  );

  const load = useCallback(async () => {
    if (!svnService) {
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const result = await svnService.listShelves(request);
      if (result.ok) {
        setShelves(result.shelves);
      } else {
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
  }, [intl, request, svnService]);

  useEffect(() => {
    if (!open) {
      return;
    }
    setName("");
    setMessage("");
    void load();
  }, [load, open]);

  const createShelf = async () => {
    if (!svnService || selectedPaths.length === 0) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const result = await runWithCredentialRetry(() =>
        svnService.shelve({
          ...request,
          paths: selectedPaths,
          ...(name.trim() ? { name: name.trim() } : {}),
          message,
        }),
      );
      if (result.ok) {
        toast(intl.formatMessage({ id: "svn.shelf.created" }, { name: result.shelf.name }));
        setName("");
        setMessage("");
        await load();
        refresh();
      } else if (result.issue !== "cancelled") {
        setError(
          intl.formatMessage(
            { id: getSvnIssueMessageId(result.issue) },
            { message: result.message },
          ),
        );
      }
    } catch (createError) {
      setError(createError instanceof Error ? createError.message : String(createError));
    } finally {
      setBusy(false);
    }
  };

  const applyShelf = async (shelf: SvnShelf) => {
    if (!svnService) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const result = await svnService.unshelve({ ...request, name: shelf.name });
      if (result.ok) {
        toast(intl.formatMessage({ id: "svn.shelf.applied" }, { name: shelf.name }));
        await load();
        refresh();
      } else if (result.issue !== "cancelled") {
        setError(
          intl.formatMessage(
            { id: getSvnIssueMessageId(result.issue) },
            { message: result.message },
          ),
        );
      }
    } finally {
      setBusy(false);
    }
  };

  const deleteShelf = async (shelf: SvnShelf) => {
    if (!svnService) {
      return;
    }
    const confirmed = await confirm({
      title: intl.formatMessage({ id: "svn.shelf.confirmDeleteTitle" }, { name: shelf.name }),
      description: intl.formatMessage({ id: "svn.shelf.confirmDeleteDescription" }),
      confirmLabel: intl.formatMessage({ id: "svn.shelf.delete" }),
    });
    if (!confirmed) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const result = await svnService.deleteShelf({ ...request, name: shelf.name });
      if (result.ok) {
        toast(intl.formatMessage({ id: "svn.shelf.deleted" }, { name: shelf.name }));
        await load();
      } else if (result.issue !== "cancelled") {
        setError(
          intl.formatMessage(
            { id: getSvnIssueMessageId(result.issue) },
            { message: result.message },
          ),
        );
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={(next) => !busy && onOpenChange(next)}>
      <DialogContent
        data-testid={TID_SVN_SHELVES_DIALOG}
        className="flex max-h-[min(78vh,720px)] max-w-2xl flex-col gap-0 overflow-hidden rounded-2xl p-0"
      >
        <DialogHeader className="border-b border-border px-5 py-4">
          <DialogTitle className="text-ui-lg font-medium text-foreground">
            {intl.formatMessage({ id: "svn.shelf.title" })}
          </DialogTitle>
        </DialogHeader>
        <div className="space-y-2 border-b border-border px-5 py-4">
          <div className="flex flex-wrap items-center gap-2">
            <Input
              data-testid={TID_SVN_SHELF_NAME}
              value={name}
              disabled={busy}
              placeholder={intl.formatMessage({ id: "svn.shelf.namePlaceholder" })}
              onChange={(event) => setName(event.target.value)}
            />
            <Input
              value={message}
              disabled={busy}
              placeholder={intl.formatMessage({ id: "svn.shelf.messagePlaceholder" })}
              onChange={(event) => setMessage(event.target.value)}
            />
            <Button
              type="button"
              data-testid={TID_SVN_SHELVE_SUBMIT}
              size="lg"
              disabled={busy || selectedPaths.length === 0}
              onClick={() => void createShelf()}
            >
              {busy ? <LoaderIcon className="animate-spin" /> : null}
              {intl.formatMessage({ id: "svn.shelf.create" }, { count: selectedPaths.length })}
            </Button>
          </div>
          {error ? (
            <p className="whitespace-pre-line text-ui-base text-destructive">{error}</p>
          ) : null}
        </div>
        <div className="min-h-0 flex-1 overflow-auto p-2">
          {loading ? (
            <p className="px-3 py-3 text-ui-base text-foreground-subtle">
              {intl.formatMessage({ id: "common.loading" })}
            </p>
          ) : shelves.length === 0 ? (
            <p className="px-3 py-3 text-ui-base text-foreground-subtle">
              {intl.formatMessage({ id: "svn.shelf.empty" })}
            </p>
          ) : (
            shelves.map((shelf) => (
              <div
                key={shelf.name}
                data-svn-shelf={shelf.name}
                className="flex flex-wrap items-center gap-2 rounded-lg px-3 py-2 hover:bg-surface-hover"
              >
                <div className="min-w-0 flex-1">
                  <p className="truncate text-ui-base text-foreground">{shelf.name}</p>
                  <p className="truncate text-ui-sm text-foreground-subtle">
                    {intl.formatMessage(
                      { id: "svn.shelf.itemMeta" },
                      {
                        count: shelf.paths.length,
                        time: dateFormatter.format(new Date(shelf.createdAt)),
                      },
                    )}
                    {shelf.message ? ` · ${shelf.message}` : ""}
                  </p>
                </div>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  data-svn-shelf-action="apply"
                  disabled={busy}
                  onClick={() => void applyShelf(shelf)}
                >
                  {intl.formatMessage({ id: "svn.shelf.apply" })}
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  data-svn-shelf-action="delete"
                  disabled={busy}
                  onClick={() => void deleteShelf(shelf)}
                >
                  {intl.formatMessage({ id: "svn.shelf.delete" })}
                </Button>
              </div>
            ))
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
