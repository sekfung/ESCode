// 知识条目详情：正文、来源定位、冲突、审核历史，以及用户显式的治理操作（spec §11）。
import type { ReactNode } from "react";
import { FileText, X } from "lucide-react";
import type { V4WikiEntryResult, V4WikiGraphNode } from "@escode/shared/escode-protocol-v4";
import { Button } from "@/components/ui/button.js";
import { cn } from "@/components/lib/utils.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import { wikiEntryActions, type WikiEntryAction } from "@/lib/wikiGraph.js";

function Badge({
  children,
  tone = "neutral",
}: {
  children: string;
  tone?: "neutral" | "warning" | "destructive" | "success";
}) {
  return (
    <span
      className={cn(
        "rounded-sm px-1.5 py-0.5 text-ui-xs font-medium",
        tone === "neutral" && "bg-surface text-foreground-subtle",
        tone === "warning" && "bg-warning/15 text-warning",
        tone === "destructive" && "bg-destructive/15 text-destructive",
        tone === "success" && "bg-success/15 text-success",
      )}
    >
      {children}
    </span>
  );
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="space-y-1.5">
      <h4 className="text-ui-sm font-medium text-foreground-subtle">{title}</h4>
      {children}
    </section>
  );
}

function statusTone(status: string): "neutral" | "warning" | "destructive" | "success" {
  if (status === "approved") return "success";
  if (status === "proposal" || status === "needs-review") return "warning";
  if (status === "rejected") return "destructive";
  return "neutral";
}

function sourceTone(status: string): "neutral" | "warning" | "destructive" {
  if (status === "current") return "neutral";
  if (status === "changed" || status === "moved") return "warning";
  return "destructive";
}

export function KnowledgeEntryDetail({
  node,
  detail,
  loading,
  busy,
  canManage,
  onClose,
  onReview,
}: {
  node: V4WikiGraphNode;
  detail: V4WikiEntryResult | null;
  loading: boolean;
  busy: boolean;
  canManage: boolean;
  onClose: () => void;
  onReview: (action: WikiEntryAction, id: string) => void;
}) {
  const { intl } = useESCodeIntl();
  const t = (id: string, values?: Record<string, string | number>) =>
    intl.formatMessage({ id }, values);

  const header = (
    <div className="flex items-start gap-2">
      <div className="min-w-0 flex-1">
        <div className="text-ui-base font-medium break-words text-foreground">{node.label}</div>
        <div className="mt-0.5 truncate font-mono text-ui-xs text-foreground-subtlest">
          {node.entryId ?? node.id}
        </div>
      </div>
      <Button variant="ghost" size="icon-sm" onClick={onClose} aria-label={t("common.close")}>
        <X />
      </Button>
    </div>
  );

  if (node.type === "source") {
    return (
      <div className="space-y-3 p-4" data-testid="knowledge-entry-detail">
        {header}
        <div className="flex flex-wrap gap-1.5">
          <Badge>{t(`settings.knowledge.scope.${node.scope}`)}</Badge>
          {node.format ? <Badge>{node.format}</Badge> : null}
          {node.sourceStatus ? (
            <Badge tone={sourceTone(node.sourceStatus)}>{node.sourceStatus}</Badge>
          ) : null}
        </div>
        <p className="text-ui-sm text-foreground-subtle">{t("settings.knowledge.sourceHint")}</p>
      </div>
    );
  }

  const entry = detail?.found ? detail.entry : undefined;
  return (
    <div className="space-y-4 p-4" data-testid="knowledge-entry-detail">
      {header}
      {loading && !entry ? (
        <div className="text-ui-sm text-foreground-subtle">{t("common.loading")}</div>
      ) : !entry ? (
        <div className="text-ui-sm text-foreground-subtle">
          {t("settings.knowledge.entryMissing")}
        </div>
      ) : (
        <>
          <div className="flex flex-wrap gap-1.5">
            <Badge>{t(`settings.knowledge.scope.${entry.scope}`)}</Badge>
            <Badge>{entry.kind}</Badge>
            <Badge tone={statusTone(entry.status)}>
              {t(`settings.knowledge.status.${entry.status}`)}
            </Badge>
            <Badge>{entry.authority}</Badge>
            {entry.validity !== "valid" ? <Badge tone="warning">{entry.validity}</Badge> : null}
          </div>
          {entry.validity_reason ? (
            <p className="text-ui-sm text-warning">{entry.validity_reason}</p>
          ) : null}
          {canManage && wikiEntryActions(entry.scope, entry.status).length > 0 ? (
            <div className="flex flex-wrap gap-2" data-testid="knowledge-entry-actions">
              {wikiEntryActions(entry.scope, entry.status).map((action) => (
                <Button
                  key={action}
                  size="sm"
                  variant={
                    action === "approve"
                      ? "default"
                      : action === "deprecate"
                        ? "warning"
                        : "outline"
                  }
                  disabled={busy}
                  data-testid={`knowledge-entry-${action}`}
                  onClick={() => onReview(action, entry.id)}
                >
                  {t(`settings.knowledge.action.${action}`)}
                </Button>
              ))}
            </div>
          ) : null}
          <Section title={t("settings.knowledge.body")}>
            <div className="max-h-72 overflow-auto rounded-lg bg-surface p-3 text-ui-sm leading-6 whitespace-pre-wrap text-foreground">
              {entry.body.trim() || "—"}
            </div>
          </Section>
          <Section title={t("settings.knowledge.sources", { count: entry.sources.length })}>
            {entry.sources.length === 0 ? (
              <div className="text-ui-sm text-foreground-subtlest">
                {t("settings.knowledge.noSources")}
              </div>
            ) : (
              <ul className="space-y-1.5">
                {entry.sources.map((source) => (
                  <li
                    key={`${source.document_id}:${source.locator}`}
                    className="rounded-lg border border-border p-2"
                  >
                    <div className="flex min-w-0 items-center gap-1.5">
                      <FileText className="size-3.5 shrink-0 text-foreground-subtle" />
                      <span
                        className="min-w-0 flex-1 truncate font-mono text-ui-sm text-foreground"
                        title={source.source_path}
                      >
                        {source.source_path}
                      </span>
                      <Badge tone={sourceTone(source.source_status)}>{source.source_status}</Badge>
                    </div>
                    <div className="mt-1 font-mono text-ui-xs text-foreground-subtle">
                      {source.format} · {source.locator}
                    </div>
                    {source.excerpt ? (
                      <div className="mt-1 line-clamp-3 text-ui-xs text-foreground-subtlest">
                        {source.excerpt}
                      </div>
                    ) : null}
                  </li>
                ))}
              </ul>
            )}
          </Section>
          {detail?.conflicts && detail.conflicts.length > 0 ? (
            <Section title={t("settings.knowledge.conflicts", { count: detail.conflicts.length })}>
              <ul className="space-y-1.5">
                {detail.conflicts.map((conflict, index) => (
                  <li
                    key={`${conflict.kind}:${index}`}
                    className="rounded-lg border border-destructive/40 p-2 text-ui-sm"
                  >
                    <div className="font-medium text-destructive">{conflict.kind}</div>
                    <div className="mt-0.5 text-foreground-subtle">
                      {conflict.parties.map((p) => p.id).join(" ↔ ")}
                    </div>
                    <div className="mt-1 text-foreground">{conflict.evidence}</div>
                    <div className="mt-1 text-foreground-subtlest">{conflict.suggestion}</div>
                  </li>
                ))}
              </ul>
            </Section>
          ) : null}
          <Section title={t("settings.knowledge.review")}>
            {detail?.review && detail.review.length > 0 ? (
              <ol className="space-y-1">
                {detail.review.map((record, index) => (
                  <li key={`${record.at}:${index}`} className="text-ui-sm text-foreground-subtle">
                    <span className="font-medium text-foreground">{record.action}</span> ·{" "}
                    {record.by} · {record.at}
                    {record.note ? (
                      <span className="text-foreground-subtlest"> — {record.note}</span>
                    ) : null}
                  </li>
                ))}
              </ol>
            ) : (
              <div className="text-ui-sm text-foreground-subtlest">
                {t("settings.knowledge.noReview")}
              </div>
            )}
          </Section>
          <div className="text-ui-xs text-foreground-subtlest">
            {t("settings.knowledge.updatedAt", { at: entry.updated_at })}
            {entry.tags.length ? ` · ${entry.tags.join(", ")}` : ""}
          </div>
        </>
      )}
    </div>
  );
}
