import { useCallback, useMemo, useState, type ReactNode } from "react";
import { AlertTriangleIcon, RefreshCwIcon } from "lucide-react";
import type { CircuitLayoutSheet, IServiceAccessor } from "@escode/services";
import type { CircuitCorrectionOp } from "@escode/shared/circuit";
import { Button } from "@/components/ui/button.js";
import { toast } from "@/components/ui/toast.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import { cn } from "@/components/lib/utils.js";
import { logger } from "@/logger.js";
import { useCircuitPanel } from "@/hooks/useCircuitPanel.js";
import { usePlatform } from "@/hooks/usePlatform.js";
import { buildHighlight, collectLowConfidenceIds, type CircuitSelection } from "@/circuit/domain/circuitView.js";
import {
  circuitComponentDraftReference,
  circuitNetDraftReference,
} from "@/circuit/domain/circuitReferences.js";
import {
  dispatchCircuitDraftReference,
  getCircuitDraftReferences,
} from "@/lib/circuit/circuitReferenceDrafts.js";
import { CircuitSchematicView } from "@/circuit/components/CircuitSchematicView.js";
import { CircuitSidePanel } from "@/circuit/components/CircuitSidePanel.js";

/**
 * 电路面板（规格 §8，Desktop）。数据面在 useCircuitPanel：这里只做选择、高亮、修正与引用 chip。
 * 面板不持有 IR 事实：渲染与列表都来自 hooks 读到的服务快照，修正走 correct(baseRevision)。
 */

export interface CircuitPanelWorkspace {
  workspacePath: string;
  workspaceIdentity?: string;
  remoteSessionId?: string;
}

export function CircuitPanel({
  services,
  workspace,
  path,
  title,
  snapshotText,
  requestedIrId,
  enabled = true,
}: {
  services: IServiceAccessor;
  workspace: CircuitPanelWorkspace | null;
  /** workspace 相对路径（服务契约要求）。 */
  path: string | undefined;
  title?: string;
  /** `.circuit.json` 导出快照文本（PreviewPane 已读到的内容，只读展示）。 */
  snapshotText?: string | null;
  /** `::circuit` 卡片带来的 irId：面板展示该源文件当前的 IR，并把它标在 DOM 上供定位核对。 */
  requestedIrId?: string;
  enabled?: boolean;
}) {
  const { intl } = useESCodeIntl();
  const platform = usePlatform();
  const circuitAvailable = platform.supportsCircuit === true;
  const { state, refresh, reparse, retry, correct, dropCorrection, dismissWriteConflict } =
    useCircuitPanel({
      // Web / 手机远控没有电路服务（§3.5）：能力位为假时不读、不订阅、也不改写。
      service: circuitAvailable ? services.circuitService : undefined,
      workspace,
      path,
      snapshotText,
      enabled,
    });
  const [selection, setSelection] = useState<CircuitSelection>({ sheetPath: "" });
  const [activeSheetPath, setActiveSheetPath] = useState<string>("");
  const [busy, setBusy] = useState(false);

  const data = state.data;
  const readOnly = state.readOnly || !circuitAvailable || !services.circuitService;
  const components = data?.components ?? [];
  const nets = data?.nets ?? [];
  const lowConfidenceIds = useMemo(
    () => collectLowConfidenceIds(components, nets),
    [components, nets],
  );
  const highlight = useMemo(
    () => buildHighlight(selection, components, nets),
    [components, nets, selection],
  );
  const sheets: CircuitLayoutSheet[] = data?.layout?.sheets ?? [];
  const activeSheet = useMemo(() => {
    if (sheets.length === 0) return undefined;
    return sheets.find((sheet) => sheet.path === activeSheetPath) ?? sheets[0];
  }, [activeSheetPath, sheets]);

  const selectComponent = useCallback(
    (componentId: string) => setSelection({ componentId, sheetPath: activeSheetPath }),
    [activeSheetPath],
  );
  const selectNet = useCallback(
    (netId: string) => setSelection({ netId, sheetPath: activeSheetPath }),
    [activeSheetPath],
  );
  const selectNetByName = useCallback(
    (name: string) => {
      const match = nets.find((net) => net.name === name) ?? nets.find((net) => net.id === name);
      if (match) selectNet(match.id);
    },
    [nets, selectNet],
  );

  const insertReference = useCallback(() => {
    if (!data) return;
    const workspaceKey = workspace?.workspaceIdentity?.trim() || workspace?.workspacePath;
    if (!workspaceKey) return;
    const selectedComponent = selection.componentId
      ? components.find((component) => component.id === selection.componentId)
      : undefined;
    const selectedNet = selection.netId
      ? nets.find((net) => net.id === selection.netId)
      : undefined;
    const reference = selectedComponent
      ? circuitComponentDraftReference(data.irId, selectedComponent)
      : selectedNet
        ? circuitNetDraftReference(data.irId, selectedNet)
        : null;
    if (!reference) return;
    const before = getCircuitDraftReferences(workspaceKey).length;
    dispatchCircuitDraftReference(workspaceKey, reference);
    logger.debug("[circuit] draft reference inserted", { irId: data.irId, kind: reference.kind });
    // chip 只是 renderer 草稿：明确告诉用户没有自动发送（§8）。
    toast(
      intl.formatMessage(
        { id: before === 0 ? "circuit.reference.inserted" : "circuit.reference.insertedMore" },
        { label: reference.label },
      ),
    );
  }, [components, data, intl, nets, selection, workspace]);

  const applyCorrections = useCallback(
    async (ops: readonly CircuitCorrectionOp[]) => {
      setBusy(true);
      try {
        const ok = await correct(ops);
        if (ok) toast(intl.formatMessage({ id: "circuit.correct.applied" }));
      } finally {
        setBusy(false);
      }
    },
    [correct, intl],
  );

  const panelTitle = title ?? path ?? "";
  if (state.status === "unavailable")
    return (
      <PanelMessage title={panelTitle} testId="circuit-panel-unavailable">
        {intl.formatMessage({ id: "circuit.unavailable" })}
      </PanelMessage>
    );
  if (state.status === "recognition")
    return (
      <PanelMessage title={panelTitle} testId="circuit-panel-recognition">
        {intl.formatMessage({ id: "circuit.recognitionNeeded" })}
      </PanelMessage>
    );
  if (state.status === "unsupported")
    return (
      <PanelMessage title={panelTitle} testId="circuit-panel-unsupported">
        {intl.formatMessage({ id: "circuit.unsupported" })}
      </PanelMessage>
    );
  if (state.status === "empty" || state.status === "error")
    return (
      <PanelMessage title={panelTitle} testId="circuit-panel-empty">
        <span className="flex flex-col items-start gap-2">
          <span>
            {intl.formatMessage(
              { id: `circuit.error.${state.error?.code ?? "io_error"}` },
              { message: state.error?.message ?? "" },
            )}
          </span>
          <Button
            type="button"
            size="sm"
            variant="outline"
            onClick={() => retry()}
            data-testid="circuit-panel-retry"
          >
            {intl.formatMessage({ id: "circuit.retryParse" })}
          </Button>
        </span>
      </PanelMessage>
    );
  if (state.status === "loading" || !data)
    return (
      <PanelMessage title={panelTitle} testId="circuit-panel-loading">
        {intl.formatMessage({ id: "circuit.loading" })}
      </PanelMessage>
    );

  return (
    <section
      className="flex h-full min-h-0 w-full flex-col overflow-hidden bg-background"
      data-testid="circuit-panel"
      data-circuit-ir-id={data.irId}
      data-circuit-requested-ir-id={requestedIrId}
      data-circuit-revision={data.revision}
      data-circuit-stale={data.stale ? "true" : "false"}
      data-circuit-read-only={readOnly ? "true" : "false"}
    >
      <div className="flex h-10 shrink-0 items-center gap-2 border-b border-border px-3">
        <span className="min-w-0 flex-1 truncate text-ui-base">{panelTitle}</span>
        <span
          className="shrink-0 rounded-sm bg-surface px-1 font-mono text-ui-xs text-foreground-subtle"
          data-testid="circuit-revision"
        >
          r{data.revision}
        </span>
        <span className="shrink-0 text-ui-xs text-foreground-subtlest">
          {intl.formatMessage(
            { id: "circuit.summary.counts" },
            { components: data.summary.componentCount, nets: data.summary.netCount },
          )}
        </span>
        {!readOnly && !data.stale ? (
          <Button
            type="button"
            size="icon-sm"
            variant="ghost"
            title={intl.formatMessage({ id: "circuit.refresh" })}
            aria-label={intl.formatMessage({ id: "circuit.refresh" })}
            onClick={() => void refresh()}
          >
            <RefreshCwIcon className="size-3.5" />
          </Button>
        ) : null}
      </div>
      {data.stale ? (
        <div
          className="flex shrink-0 items-center gap-2 border-b border-warning/40 bg-warning/10 px-3 py-1.5 text-ui-sm text-warning"
          data-testid="circuit-stale-banner"
        >
          <AlertTriangleIcon className="size-3.5 shrink-0" />
          <span className="min-w-0 flex-1">{intl.formatMessage({ id: "circuit.stale.hint" })}</span>
          {!readOnly ? (
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={busy}
              onClick={() => void reparse()}
              data-testid="circuit-reparse"
            >
              {intl.formatMessage({ id: "circuit.stale.reparse" })}
            </Button>
          ) : null}
        </div>
      ) : null}
      {state.writeConflict ? (
        <div
          className="flex shrink-0 items-center gap-2 border-b border-destructive/40 bg-destructive/10 px-3 py-1.5 text-ui-sm text-destructive"
          data-testid="circuit-write-conflict"
        >
          <AlertTriangleIcon className="size-3.5 shrink-0" />
          <span className="min-w-0 flex-1">
            {intl.formatMessage({ id: "circuit.staleRevision.hint" })}
          </span>
          <Button
            type="button"
            size="sm"
            variant="outline"
            onClick={() => {
              dismissWriteConflict();
              void refresh();
            }}
            data-testid="circuit-write-conflict-refresh"
          >
            {intl.formatMessage({ id: "circuit.refresh" })}
          </Button>
        </div>
      ) : null}
      {state.error && state.status === "ready" ? (
        <div
          className="shrink-0 border-b border-destructive/40 bg-destructive/10 px-3 py-1.5 text-ui-sm text-destructive"
          data-testid="circuit-error-banner"
        >
          {intl.formatMessage(
            { id: `circuit.error.${state.error.code ?? "io_error"}` },
            { message: state.error.message },
          )}
        </div>
      ) : null}
      <div className="flex min-h-0 flex-1">
        <div className="flex min-h-0 min-w-0 flex-1 flex-col">
          {sheets.length > 1 ? (
            <div className="flex shrink-0 flex-wrap gap-1 border-b border-border px-2 py-1">
              {sheets.map((sheet) => (
                <Button
                  key={sheet.path || "root"}
                  type="button"
                  size="xs"
                  variant={sheet === activeSheet ? "secondary" : "ghost"}
                  onClick={() => {
                    setActiveSheetPath(sheet.path);
                    setSelection({ sheetPath: sheet.path });
                  }}
                  data-testid="circuit-sheet-tab"
                  data-circuit-sheet-path={sheet.path}
                >
                  {sheet.name ?? (sheet.path || intl.formatMessage({ id: "circuit.sheet.root" }))}
                </Button>
              ))}
            </div>
          ) : null}
          <div className="min-h-0 flex-1 overflow-auto bg-surface/20 p-2">
            {activeSheet ? (
              <CircuitSchematicView
                sheet={activeSheet}
                highlight={highlight}
                lowConfidenceIds={lowConfidenceIds}
                selectedComponentId={selection.componentId}
                onSelectComponent={selectComponent}
                onSelectNetName={selectNetByName}
                labels={{
                  schematic: intl.formatMessage({ id: "circuit.schematic.aria" }),
                  lowConfidence: intl.formatMessage({ id: "circuit.lowConfidence" }),
                }}
              />
            ) : (
              <p className="p-3 text-ui-sm text-foreground-subtle" data-testid="circuit-no-layout">
                {intl.formatMessage({ id: "circuit.noLayout" })}
              </p>
            )}
          </div>
          {data.truncated ? (
            <p className="shrink-0 px-3 py-1 text-ui-xs text-foreground-subtlest">
              {intl.formatMessage({ id: "circuit.truncated" })}
            </p>
          ) : null}
        </div>
        <CircuitSidePanel
          data={data}
          selection={selection}
          readOnly={readOnly}
          busy={busy}
          onSelectComponent={selectComponent}
          onSelectNet={selectNet}
          onDropCorrection={(index) => void dropCorrection(index)}
          onSubmitCorrections={(ops) => void applyCorrections(ops)}
          onInsertReference={insertReference}
        />
      </div>
    </section>
  );
}

function PanelMessage({
  title,
  testId,
  children,
}: {
  title: string;
  testId: string;
  children: ReactNode;
}) {
  return (
    <section
      className={cn("flex h-full w-full flex-col overflow-hidden bg-background")}
      data-testid={testId}
    >
      <div className="flex h-10 shrink-0 items-center border-b border-border px-3">
        <span className="min-w-0 flex-1 truncate text-ui-base">{title}</span>
      </div>
      <div className="min-h-0 flex-1 overflow-auto p-4 text-ui-sm text-foreground-subtle">
        {children}
      </div>
    </section>
  );
}
