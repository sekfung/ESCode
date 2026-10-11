import { useCallback, useMemo, useState } from "react";
import { SendIcon } from "lucide-react";
import type { CircuitCorrectionOp } from "@escode/shared/circuit";
import { Button } from "@/components/ui/button.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import {
  buildHighlight,
  collectLowConfidenceIds,
  netLabel,
  type CircuitSelection,
} from "@/circuit/domain/circuitView.js";
import type { CircuitPanelData } from "@/hooks/useCircuitPanel.js";
import { CircuitBomList, CircuitNetList } from "@/circuit/components/CircuitSideLists.js";
import { CircuitCorrectionsPanel } from "@/circuit/components/CircuitCorrectionsPanel.js";
import { CircuitEditForm } from "@/circuit/components/CircuitEditForm.js";

/**
 * 面板侧栏（规格 §8）：BOM / 网络表 / 修正记录三个页签 + 选中项的「插入到输入框」。
 * 与 SVG 共用同一份 selection：点器件高亮整条连接时，这里的选中行同步跟着走。
 */

export type CircuitSideTab = "bom" | "nets" | "corrections";

export function CircuitSidePanel({
  data,
  selection,
  readOnly,
  busy,
  onSelectComponent,
  onSelectNet,
  onDropCorrection,
  onSubmitCorrections,
  onInsertReference,
}: {
  data: CircuitPanelData;
  selection: CircuitSelection;
  readOnly: boolean;
  busy: boolean;
  onSelectComponent: (componentId: string) => void;
  onSelectNet: (netId: string) => void;
  onDropCorrection: (index: number) => void;
  onSubmitCorrections: (ops: readonly CircuitCorrectionOp[]) => void;
  onInsertReference: () => void;
}) {
  const { intl } = useESCodeIntl();
  const [sideTab, setSideTab] = useState<CircuitSideTab>("bom");
  const components = data.components;
  const nets = data.nets;
  const componentsById = useMemo(
    () => new Map(components.map((component) => [component.id, component])),
    [components],
  );
  const lowConfidenceIds = useMemo(
    () => collectLowConfidenceIds(components, nets),
    [components, nets],
  );
  const highlight = useMemo(
    () => buildHighlight(selection, components, nets),
    [components, nets, selection],
  );
  const selectedComponent = selection.componentId
    ? componentsById.get(selection.componentId)
    : undefined;
  const selectedNet = selection.netId ? nets.find((net) => net.id === selection.netId) : undefined;

  const selectComponent = useCallback(
    (componentId: string) => {
      setSideTab((current) => (current === "nets" ? "bom" : current));
      onSelectComponent(componentId);
    },
    [onSelectComponent],
  );

  return (
    <aside
      className="flex min-h-0 w-80 shrink-0 flex-col border-l border-border"
      data-testid="circuit-side-panel"
    >
      <div className="flex shrink-0 items-center gap-1 border-b border-border px-2 py-1">
        {(["bom", "nets", "corrections"] as const).map((tab) => (
          <Button
            key={tab}
            type="button"
            size="xs"
            variant={sideTab === tab ? "secondary" : "ghost"}
            onClick={() => setSideTab(tab)}
            data-testid={`circuit-side-tab-${tab}`}
          >
            {intl.formatMessage({ id: `circuit.side.${tab}` })}
          </Button>
        ))}
      </div>
      <div className="min-h-0 flex-1 overflow-auto py-1">
        {sideTab === "bom" ? (
          <CircuitBomList
            components={components}
            lowConfidenceIds={lowConfidenceIds}
            selectedComponentId={selection.componentId}
            onSelect={selectComponent}
          />
        ) : null}
        {sideTab === "nets" ? (
          <CircuitNetList
            nets={nets}
            componentsById={componentsById}
            lowConfidenceIds={lowConfidenceIds}
            selectedNetId={selection.netId}
            onSelect={onSelectNet}
          />
        ) : null}
        {sideTab === "corrections" ? (
          <CircuitCorrectionsPanel
            corrections={data.corrections}
            readOnly={readOnly}
            busy={busy}
            onDrop={onDropCorrection}
          />
        ) : null}
      </div>
      {sideTab === "bom" && selectedComponent && !readOnly ? (
        <CircuitEditForm
          key={`${selectedComponent.id}:${data.revision}`}
          component={selectedComponent}
          nets={nets}
          disabled={busy}
          onSubmit={onSubmitCorrections}
        />
      ) : null}
      <div className="flex shrink-0 flex-col gap-1 border-t border-border p-2">
        <span className="text-ui-xs text-foreground-subtlest" data-testid="circuit-selection">
          {selectedComponent
            ? selectedComponent.ref
            : selectedNet
              ? netLabel(selectedNet)
              : intl.formatMessage({ id: "circuit.selection.none" })}
        </span>
        {selectedComponent ? (
          <span className="text-ui-xs text-foreground-subtlest">
            {intl.formatMessage(
              { id: "circuit.selection.nets" },
              { count: highlight.netIds.size },
            )}
          </span>
        ) : null}
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={readOnly || (!selectedComponent && !selectedNet)}
          onClick={onInsertReference}
          data-testid="circuit-insert-reference"
        >
          <SendIcon className="size-3.5" data-icon="inline-start" />
          {intl.formatMessage({ id: "circuit.reference.insert" })}
        </Button>
      </div>
    </aside>
  );
}
