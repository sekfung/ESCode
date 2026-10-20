// 知识库：LLM Wiki 知识图谱（apps/escode-cli-rust/docs/llm-wiki/01-spec.md §11）。
// 主视图入口：workspace 侧边栏「知识库」（搜索入口下方）；设置页不再提供该分区（settingsNavigation 中隐藏）。
// 只展示 runtime 的事实快照；导入、批准/拒绝/弃用、提升到全局都需要用户显式操作并经确认，由 runtime 落盘与审计。
import { useCallback, useMemo, useState, type ReactNode } from "react";
import { FileUp, RefreshCw } from "lucide-react";
import {
  WIKI_KINDS,
  WIKI_SCOPES,
  WIKI_STATUSES,
  type V4WikiGraphNode,
  type V4WikiStatusResult,
} from "@escode/shared/escode-protocol-v4";
import { Button } from "@/components/ui/button.js";
import { Switch } from "@/components/ui/switch.js";
import { toast } from "@/components/ui/toast.js";
import { cn } from "@/components/lib/utils.js";
import { useConfirmDialog } from "@/hooks/useConfirmDialog.js";
import { useOptionalPlatform } from "@/hooks/usePlatform.js";
import {
  DEFAULT_WIKI_GRAPH_FILTERS,
  useWikiKnowledge,
  type WikiGraphFilters,
  type WikiKnowledgePhase,
  type WikiKnowledgeTarget,
} from "@/hooks/useWikiKnowledge.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import { WIKI_TONE_CLASS, wikiKindTone, type WikiEntryAction } from "@/lib/wikiGraph.js";
import { canPickImportPaths } from "@/lib/wikiImport.js";
import { KnowledgeEntryDetail } from "@/settings/KnowledgeEntryDetail.js";
import { KnowledgeGraphCanvas } from "@/settings/KnowledgeGraphCanvas.js";
import {
  KnowledgeImportDialog,
  type KnowledgeImportRequest,
} from "@/settings/KnowledgeImportDialog.js";

type Translate = (id: string, values?: Record<string, string | number>) => string;

function toggle<T>(list: readonly T[], value: T): T[] {
  return list.includes(value) ? list.filter((item) => item !== value) : [...list, value];
}

function FilterChip({
  active,
  label,
  onClick,
  testId,
}: {
  active: boolean;
  label: string;
  onClick: () => void;
  testId?: string;
}) {
  return (
    <button
      type="button"
      aria-pressed={active}
      data-testid={testId}
      onClick={onClick}
      className={cn(
        "h-6 rounded-md border px-2 text-ui-sm transition-colors",
        active
          ? "border-brand bg-accent text-foreground"
          : "border-border text-foreground-subtle hover:border-border-hover hover:bg-hover",
      )}
    >
      {label}
    </button>
  );
}

function FilterRow({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5 sm:flex-row sm:items-start sm:gap-3">
      <div className="w-16 shrink-0 pt-0.5 text-ui-sm text-foreground-subtle">{title}</div>
      <div className="flex min-w-0 flex-wrap gap-1.5">{children}</div>
    </div>
  );
}

/** 非 ready 阶段的整页提示；返回 null 表示应显示图谱。 */
export function KnowledgePhaseNotice({
  phase,
  error,
  status,
  t,
  onRetry,
}: {
  phase: WikiKnowledgePhase;
  error: string | null;
  status: V4WikiStatusResult | null;
  t: Translate;
  onRetry: () => void;
}) {
  if (phase === "ready") return null;
  const messageId: Record<Exclude<WikiKnowledgePhase, "ready">, string> = {
    "no-workspace": "settings.knowledge.noWorkspace",
    waiting: "settings.knowledge.waiting",
    loading: "common.loading",
    disabled: "settings.knowledge.disabled",
    unsupported: "settings.knowledge.unsupported",
    error: "settings.knowledge.loadFailed",
  };
  return (
    <div
      className="rounded-xl border border-border bg-card p-6 text-center"
      data-testid="knowledge-phase-notice"
      data-phase={phase}
    >
      <div className="text-ui-base text-foreground">{t(messageId[phase])}</div>
      {phase === "unsupported" ? (
        <div className="mt-1 text-ui-sm text-foreground-subtle">
          {t("settings.knowledge.unsupportedHint")}
        </div>
      ) : null}
      {phase === "disabled" ? (
        <div className="mt-1 font-mono text-ui-sm text-foreground-subtle">
          {'"wiki": { "enabled": true }'}
        </div>
      ) : null}
      {phase === "disabled" && status?.configIssues?.length ? (
        <ul className="mt-2 text-ui-sm text-warning">
          {status.configIssues.map((issue) => (
            <li key={issue}>{issue}</li>
          ))}
        </ul>
      ) : null}
      {phase === "error" ? (
        <>
          {error ? (
            <div className="mt-1 text-ui-sm break-words text-destructive">{error}</div>
          ) : null}
          <Button className="mt-3" size="sm" variant="outline" onClick={onRetry}>
            {t("common.retry")}
          </Button>
        </>
      ) : null}
    </div>
  );
}

function Summary({ status, t }: { status: V4WikiStatusResult; t: Translate }) {
  return (
    <div
      className="flex flex-wrap items-center gap-x-4 gap-y-1 text-ui-sm text-foreground-subtle"
      data-testid="knowledge-summary"
    >
      {status.project ? (
        <span className="min-w-0 truncate">
          {t("settings.knowledge.project")}:{" "}
          <span className="font-medium text-foreground">{status.project.name}</span>
          {status.project.identityKind ? ` (${status.project.identityKind})` : ""}
        </span>
      ) : null}
      {(status.scopes ?? []).map((scope) => (
        <span key={scope.scope}>
          {t("settings.knowledge.entryCount", {
            scope: t(`settings.knowledge.scope.${scope.scope}`),
            count: scope.entries,
          })}
        </span>
      ))}
      {status.conflicts ? (
        <span className="text-destructive">
          {t("settings.knowledge.conflicts", { count: status.conflicts })}
        </span>
      ) : null}
    </div>
  );
}

function Legend({ t }: { t: Translate }) {
  const tones = [
    "standard",
    "architecture",
    "guide",
    "reference",
    "anti-pattern",
    "proposal",
  ] as const;
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-ui-xs text-foreground-subtle">
      {tones.map((kind) => (
        <span key={kind} className="inline-flex items-center gap-1">
          <span className={cn("size-2.5 rounded-full", WIKI_TONE_CLASS[wikiKindTone(kind)])} />
          {t(`settings.knowledge.legend.${kind}`)}
        </span>
      ))}
      <span className="inline-flex items-center gap-1">
        <span className="size-2.5 rounded-full border border-dashed border-foreground-subtle" />
        {t("settings.knowledge.status.proposal")}
      </span>
      <span className="inline-flex items-center gap-1">
        <span className="size-2.5 rounded-sm border border-border-hover bg-surface" />
        {t("settings.knowledge.legend.source")}
      </span>
    </div>
  );
}

export function KnowledgeSettingsSection(target: WikiKnowledgeTarget) {
  const { intl } = useESCodeIntl();
  const t = useCallback<Translate>((id, values) => intl.formatMessage({ id }, values), [intl]);
  const confirm = useConfirmDialog();
  const [filters, setFilters] = useState<WikiGraphFilters>(DEFAULT_WIKI_GRAPH_FILTERS);
  const wiki = useWikiKnowledge(target, filters);
  const [selectedNode, setSelectedNode] = useState<V4WikiGraphNode | null>(null);
  const [importOpen, setImportOpen] = useState(false);
  const platform = useOptionalPlatform();
  const canPickPaths = canPickImportPaths({
    canSelectFilePath: platform?.canSelectFilePath,
    remoteSessionId: target.remoteSessionId,
    remoteTarget: target.remoteTarget,
  });
  const canImport = wiki.canManage && wiki.phase === "ready";
  const handleImport = useCallback(
    (request: KnowledgeImportRequest) =>
      wiki.manage({
        action: "ingest",
        paths: request.paths,
        scope: request.scope,
        approveImported: request.approveImported,
      }),
    [wiki],
  );

  const nodes = useMemo(() => wiki.graph?.nodes ?? [], [wiki.graph]);
  const edges = useMemo(() => wiki.graph?.edges ?? [], [wiki.graph]);
  // 选中节点以最新快照为准：重拉后节点消失（例如被筛掉）则关闭详情。
  const activeNode = useMemo(
    () => (selectedNode ? (nodes.find((node) => node.id === selectedNode.id) ?? null) : null),
    [nodes, selectedNode],
  );

  const handleSelect = useCallback(
    (node: V4WikiGraphNode | null) => {
      setSelectedNode(node);
      void wiki.select(node?.type === "entry" ? (node.entryId ?? node.id) : null);
    },
    [wiki],
  );

  const handleReview = useCallback(
    async (action: WikiEntryAction, id: string) => {
      const confirmed = await confirm({
        title: t(`settings.knowledge.confirm.${action}.title`),
        description: t(`settings.knowledge.confirm.${action}.description`, { id }),
        confirmLabel: t(`settings.knowledge.action.${action}`),
        confirmVariant: action === "approve" || action === "promote" ? "default" : "destructive",
        testId: "knowledge-review-confirm",
      });
      if (!confirmed) return;
      try {
        const result = await wiki.manage({ action, ids: [id] });
        if (result)
          toast(
            result.ok
              ? result.summary
              : t("settings.knowledge.actionFailed", { summary: result.summary }),
          );
      } catch (error) {
        toast(error instanceof Error ? error.message : String(error));
      }
    },
    [confirm, t, wiki],
  );

  return (
    <div className="flex min-h-0 flex-col gap-4" data-testid="knowledge-settings-section">
      <div className="flex items-start gap-3">
        <div className="min-w-0 flex-1">
          <p className="text-ui-sm text-foreground-subtle">{t("settings.knowledge.description")}</p>
        </div>
        <Button
          variant="outline"
          size="sm"
          disabled={wiki.phase === "loading" || wiki.phase === "no-workspace"}
          onClick={() => void wiki.reload()}
          data-testid="knowledge-refresh"
        >
          <RefreshCw className={cn(wiki.phase === "loading" && "animate-spin")} />
          {t("common.refresh")}
        </Button>
        {canImport ? (
          <Button size="sm" onClick={() => setImportOpen(true)} data-testid="knowledge-import-open">
            <FileUp />
            {t("settings.knowledge.import.open")}
          </Button>
        ) : null}
      </div>
      {wiki.status && wiki.phase === "ready" ? <Summary status={wiki.status} t={t} /> : null}
      <KnowledgePhaseNotice
        phase={wiki.phase}
        error={wiki.error}
        status={wiki.status}
        t={t}
        onRetry={() => void wiki.reload()}
      />
      {wiki.phase === "ready" ? (
        <>
          <div className="space-y-2 rounded-xl border border-border bg-card p-3">
            <FilterRow title={t("settings.knowledge.filter.scope")}>
              {WIKI_SCOPES.map((scope) => (
                <FilterChip
                  key={scope}
                  active={filters.scopes.includes(scope)}
                  label={t(`settings.knowledge.scope.${scope}`)}
                  testId={`knowledge-filter-scope-${scope}`}
                  onClick={() => setFilters((f) => ({ ...f, scopes: toggle(f.scopes, scope) }))}
                />
              ))}
            </FilterRow>
            <FilterRow title={t("settings.knowledge.filter.status")}>
              {WIKI_STATUSES.map((status) => (
                <FilterChip
                  key={status}
                  active={filters.statuses.includes(status)}
                  label={t(`settings.knowledge.status.${status}`)}
                  onClick={() =>
                    setFilters((f) => ({ ...f, statuses: toggle(f.statuses, status) }))
                  }
                />
              ))}
            </FilterRow>
            <FilterRow title={t("settings.knowledge.filter.kind")}>
              {WIKI_KINDS.map((kind) => (
                <FilterChip
                  key={kind}
                  active={filters.kinds.includes(kind)}
                  label={kind}
                  onClick={() => setFilters((f) => ({ ...f, kinds: toggle(f.kinds, kind) }))}
                />
              ))}
            </FilterRow>
            <label className="flex items-center gap-2 text-ui-sm text-foreground-subtle">
              <Switch
                checked={filters.includeSources}
                onCheckedChange={(checked) =>
                  setFilters((f) => ({ ...f, includeSources: checked }))
                }
              />
              {t("settings.knowledge.filter.sources")}
            </label>
          </div>
          <Legend t={t} />
          {wiki.graph?.truncated ? (
            <div className="text-ui-sm text-warning">
              {t("settings.knowledge.truncated", { count: nodes.length })}
            </div>
          ) : null}
          {/* 桌面：图谱与详情左右并排；窄视口：上下布局（spec §11）。 */}
          <div className="grid min-h-0 grid-cols-1 gap-3 lg:grid-cols-[minmax(0,1fr)_360px]">
            <div className="h-[360px] overflow-hidden rounded-xl border border-border bg-background sm:h-[520px]">
              {nodes.length === 0 ? (
                <div
                  className="flex h-full flex-col items-center justify-center gap-1 p-6 text-center"
                  data-testid="knowledge-graph-empty"
                >
                  <div className="text-ui-base text-foreground">
                    {t("settings.knowledge.empty")}
                  </div>
                  {canImport ? (
                    <Button
                      size="sm"
                      className="mt-2"
                      onClick={() => setImportOpen(true)}
                      data-testid="knowledge-import-open-empty"
                    >
                      <FileUp />
                      {t("settings.knowledge.import.open")}
                    </Button>
                  ) : null}
                </div>
              ) : (
                <KnowledgeGraphCanvas
                  nodes={nodes}
                  edges={edges}
                  selectedId={activeNode?.id ?? null}
                  onSelect={handleSelect}
                  ariaLabel={t("settings.knowledge.graphLabel")}
                />
              )}
            </div>
            <div className="min-h-0 overflow-auto rounded-xl border border-border bg-card lg:h-[520px]">
              {activeNode ? (
                <KnowledgeEntryDetail
                  node={activeNode}
                  detail={wiki.entry}
                  loading={wiki.entryLoading}
                  busy={wiki.busy}
                  canManage={wiki.canManage}
                  onClose={() => handleSelect(null)}
                  onReview={(action, id) => void handleReview(action, id)}
                />
              ) : (
                <div className="p-4 text-ui-sm text-foreground-subtle">
                  {t("settings.knowledge.selectHint")}
                </div>
              )}
            </div>
          </div>
        </>
      ) : null}
      <KnowledgeImportDialog
        open={importOpen}
        onOpenChange={setImportOpen}
        status={wiki.status}
        canPickPaths={canPickPaths}
        busy={wiki.busy}
        onImport={handleImport}
      />
    </div>
  );
}
