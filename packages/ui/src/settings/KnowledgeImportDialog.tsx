// 知识库「导入文档」对话框（apps/escode-cli-rust/docs/llm-wiki/01-spec.md §11「导入入口」）。
// 只收集来源路径与目标作用域；导入、解析与落盘全部由 runtime 完成，结果按 runtime 报告逐文件展示。
import { useState } from "react";
import { CircleCheck, CircleX, File, FolderOpen, Loader2, TriangleAlert } from "lucide-react";
import type {
  V4WikiManageResult,
  V4WikiStatusResult,
  WikiScope,
} from "@escode/shared/escode-protocol-v4";
import { Button } from "@/components/ui/button.js";
import { Checkbox } from "@/components/ui/checkbox.js";
import { Dialog, DialogContent, DialogFooter, DialogTitle } from "@/components/ui/dialog.js";
import { Textarea } from "@/components/ui/textarea.js";
import { cn } from "@/components/lib/utils.js";
import { useConfirmDialog } from "@/hooks/useConfirmDialog.js";
import { useOptionalPlatform } from "@/hooks/usePlatform.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import {
  DEFAULT_IMPORT_SCOPE,
  appendImportPaths,
  importRowTone,
  parseImportPaths,
  summarizeImportRows,
  toImportRows,
  type WikiImportRow,
} from "@/lib/wikiImport.js";
import { logger } from "@/logger.js";

export interface KnowledgeImportRequest {
  paths: string[];
  scope: WikiScope;
  approveImported: boolean;
}

function scopeRoot(status: V4WikiStatusResult | null, scope: WikiScope): string | undefined {
  return status?.scopes?.find((s) => s.scope === scope)?.root;
}

function RowIcon({ tone }: { tone: ReturnType<typeof importRowTone> }) {
  if (tone === "success") return <CircleCheck className="size-4 shrink-0 text-success" />;
  if (tone === "destructive") return <CircleX className="size-4 shrink-0 text-destructive" />;
  if (tone === "warning") return <TriangleAlert className="size-4 shrink-0 text-warning" />;
  return <File className="size-4 shrink-0 text-foreground-subtle" />;
}

function ImportResult({
  result,
  rows,
  scope,
}: {
  result: V4WikiManageResult;
  rows: WikiImportRow[];
  scope: WikiScope;
}) {
  const { intl } = useESCodeIntl();
  const t = (id: string, values?: Record<string, string | number>) =>
    intl.formatMessage({ id }, values);
  const summary = summarizeImportRows(rows);
  return (
    <div className="space-y-3" data-testid="knowledge-import-result">
      <p
        className={cn("text-ui-base", result.ok ? "text-foreground" : "text-warning")}
        title={result.summary}
      >
        {t("settings.knowledge.import.summary", {
          ...summary,
          scope: t(`settings.knowledge.import.scope.${scope}.title`),
        })}
        {summary.failed || summary.skipped
          ? t("settings.knowledge.import.summaryIssues", {
              failed: summary.failed,
              skipped: summary.skipped,
            })
          : ""}
      </p>
      <ul className="max-h-80 space-y-1 overflow-auto rounded-xl border border-border p-2">
        {rows.map((row) => (
          <li
            key={row.path}
            className="flex items-start gap-2 rounded-lg px-2 py-1.5"
            data-status={row.status}
          >
            <RowIcon tone={importRowTone(row.status)} />
            <div className="min-w-0 flex-1">
              <div className="truncate font-mono text-ui-sm text-foreground" title={row.path}>
                {row.path}
              </div>
              <div className="text-ui-sm text-foreground-subtle">
                {t(`settings.knowledge.import.status.${row.status}`)}
                {row.created
                  ? ` · ${t("settings.knowledge.import.created", { count: row.created })}`
                  : ""}
                {row.updated
                  ? ` · ${t("settings.knowledge.import.updated", { count: row.updated })}`
                  : ""}
                {row.proposals
                  ? ` · ${t("settings.knowledge.import.proposals", { count: row.proposals })}`
                  : ""}
              </div>
              {row.error ? (
                <div className="text-ui-sm break-words text-destructive">{row.error}</div>
              ) : null}
            </div>
          </li>
        ))}
      </ul>
    </div>
  );
}

export function KnowledgeImportDialog({
  open,
  onOpenChange,
  status,
  canPickPaths,
  busy,
  onImport,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  status: V4WikiStatusResult | null;
  /** 本地桌面 workspace 才提供系统选择框（选出的本机路径 runtime 能读到）。 */
  canPickPaths: boolean;
  busy: boolean;
  onImport: (request: KnowledgeImportRequest) => Promise<V4WikiManageResult | null>;
}) {
  const { intl } = useESCodeIntl();
  const t = (id: string, values?: Record<string, string | number>) =>
    intl.formatMessage({ id }, values);
  const platform = useOptionalPlatform();
  const confirm = useConfirmDialog();
  const [text, setText] = useState("");
  const [scope, setScope] = useState<WikiScope>(DEFAULT_IMPORT_SCOPE);
  const [approve, setApprove] = useState(false);
  const [result, setResult] = useState<{ value: V4WikiManageResult; scope: WikiScope } | null>(
    null,
  );
  const [error, setError] = useState<string | null>(null);
  const paths = parseImportPaths(text);

  const reset = () => {
    setText("");
    setScope(DEFAULT_IMPORT_SCOPE);
    setApprove(false);
    setResult(null);
    setError(null);
  };

  const handleOpenChange = (next: boolean) => {
    // 导入进行中不允许关闭：结果需要回到这个对话框展示。
    if (!next && busy) return;
    if (!next) reset();
    onOpenChange(next);
  };

  const pick = async (kind: "files" | "folder") => {
    if (!platform) return;
    try {
      const picked =
        kind === "folder"
          ? [await platform.selectDirectory()].filter((p): p is string => Boolean(p))
          : platform.selectFiles
            ? await platform.selectFiles()
            : [await platform.selectFile()].filter((p): p is string => Boolean(p));
      if (picked.length) setText((current) => appendImportPaths(current, picked));
    } catch (pickError) {
      logger.warn("[KnowledgeImport] path picker failed", {
        error: pickError instanceof Error ? pickError.message : String(pickError),
      });
    }
  };

  const submit = async () => {
    if (!paths.length || busy) return;
    if (approve) {
      const confirmed = await confirm({
        title: t("settings.knowledge.import.confirmApprove.title"),
        description: t("settings.knowledge.import.confirmApprove.description", {
          count: paths.length,
          scope: t(`settings.knowledge.scope.${scope}`),
        }),
        confirmLabel: t("settings.knowledge.import.submit"),
        testId: "knowledge-import-approve-confirm",
      });
      if (!confirmed) return;
    }
    setError(null);
    try {
      const next = await onImport({ paths, scope, approveImported: approve });
      if (next) setResult({ value: next, scope });
    } catch (importError) {
      setError(importError instanceof Error ? importError.message : String(importError));
    }
  };

  const scopes: WikiScope[] = ["project", "global"];
  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="max-w-xl" data-testid="knowledge-import-dialog">
        <DialogTitle className="text-ui-lg font-medium">
          {t(result ? "settings.knowledge.import.doneTitle" : "settings.knowledge.import.title")}
        </DialogTitle>
        {result ? (
          <>
            <ImportResult
              result={result.value}
              rows={toImportRows(result.value.items)}
              scope={result.scope}
            />
            <DialogFooter>
              <Button data-testid="knowledge-import-done" onClick={() => handleOpenChange(false)}>
                {t("settings.knowledge.import.done")}
              </Button>
            </DialogFooter>
          </>
        ) : (
          <>
            <section className="space-y-2">
              <h3 className="text-ui-base font-medium text-foreground">
                {t("settings.knowledge.import.sources")}
              </h3>
              {canPickPaths ? (
                <div className="flex gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={busy}
                    onClick={() => void pick("files")}
                    data-testid="knowledge-import-pick-files"
                  >
                    <File />
                    {t("settings.knowledge.import.pickFiles")}
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={busy}
                    onClick={() => void pick("folder")}
                    data-testid="knowledge-import-pick-folder"
                  >
                    <FolderOpen />
                    {t("settings.knowledge.import.pickFolder")}
                  </Button>
                </div>
              ) : null}
              <Textarea
                value={text}
                disabled={busy}
                rows={4}
                onChange={(event) => setText(event.target.value)}
                placeholder={t("settings.knowledge.import.placeholder")}
                className="font-mono text-ui-sm"
                data-testid="knowledge-import-paths"
              />
              <p className="text-ui-sm text-foreground-subtle">
                {t(
                  canPickPaths
                    ? "settings.knowledge.import.pathHint"
                    : "settings.knowledge.import.remotePathHint",
                )}
              </p>
              <p className="text-ui-sm text-foreground-subtlest">
                {t("settings.knowledge.import.formats")}
              </p>
            </section>
            <section
              className="space-y-2"
              role="radiogroup"
              aria-label={t("settings.knowledge.import.target")}
            >
              <h3 className="text-ui-base font-medium text-foreground">
                {t("settings.knowledge.import.target")}
              </h3>
              {scopes.map((option) => (
                <button
                  key={option}
                  type="button"
                  role="radio"
                  aria-checked={scope === option}
                  disabled={busy}
                  onClick={() => setScope(option)}
                  data-testid={`knowledge-import-scope-${option}`}
                  className={cn(
                    "flex w-full items-start gap-3 rounded-xl border p-3 text-left transition-colors",
                    scope === option
                      ? "border-brand bg-accent"
                      : "border-border hover:border-border-hover hover:bg-hover",
                  )}
                >
                  <span
                    aria-hidden
                    className={cn(
                      "mt-1 flex size-3.5 shrink-0 items-center justify-center rounded-full border",
                      scope === option ? "border-brand" : "border-border-hover",
                    )}
                  >
                    {scope === option ? <span className="size-1.5 rounded-full bg-brand" /> : null}
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block text-ui-base font-medium text-foreground">
                      {t(`settings.knowledge.import.scope.${option}.title`)}
                    </span>
                    <span
                      className="block truncate font-mono text-ui-sm text-foreground-subtle"
                      title={scopeRoot(status, option)}
                    >
                      {scopeRoot(status, option) ??
                        t(`settings.knowledge.import.scope.${option}.location`)}
                    </span>
                    <span className="block text-ui-sm text-foreground-subtle">
                      {t(`settings.knowledge.import.scope.${option}.description`)}
                    </span>
                  </span>
                </button>
              ))}
            </section>
            <label className="flex items-start gap-2 text-ui-base text-foreground">
              <Checkbox
                checked={approve}
                disabled={busy}
                onCheckedChange={(checked) => setApprove(checked === true)}
                className="mt-0.5"
                data-testid="knowledge-import-approve"
              />
              <span>
                {t("settings.knowledge.import.approve")}
                <span className="block text-ui-sm text-foreground-subtle">
                  {t("settings.knowledge.import.approveHint")}
                </span>
              </span>
            </label>
            {error ? <p className="text-ui-sm break-words text-destructive">{error}</p> : null}
            <DialogFooter>
              <Button variant="outline" disabled={busy} onClick={() => handleOpenChange(false)}>
                {t("common.cancel")}
              </Button>
              <Button
                disabled={busy || paths.length === 0}
                onClick={() => void submit()}
                data-testid="knowledge-import-submit"
              >
                {busy ? <Loader2 className="animate-spin" /> : null}
                {t(busy ? "settings.knowledge.import.running" : "settings.knowledge.import.submit")}
              </Button>
            </DialogFooter>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}
