import { memo } from "react";
import type { CircuitComponent, CircuitNet } from "@escode/shared/circuit";
import { cn } from "@/components/lib/utils.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import { componentLabel, netLabel, pinLabel } from "@/circuit/domain/circuitView.js";

/**
 * 侧栏的 BOM 与网络表（规格 §8）：两个列表共享选中态，与 SVG 的高亮由同一个 selection 驱动。
 * 列表读的是 IR（经 hooks 的 query 全量读取），不是 layout 反推的解析快照。
 */

function LowConfidenceBadge() {
  const { intl } = useESCodeIntl();
  return (
    <span
      className="shrink-0 rounded-sm bg-warning/20 px-1 text-ui-xs text-warning"
      data-circuit-low-confidence-badge="true"
    >
      {intl.formatMessage({ id: "circuit.lowConfidence" })}
    </span>
  );
}

export const CircuitBomList = memo(function CircuitBomList({
  components,
  lowConfidenceIds,
  selectedComponentId,
  onSelect,
}: {
  components: readonly CircuitComponent[];
  lowConfidenceIds: ReadonlySet<string>;
  selectedComponentId?: string;
  onSelect?: (componentId: string) => void;
}) {
  const { intl } = useESCodeIntl();
  if (components.length === 0)
    return (
      <p className="px-2 py-1 text-ui-sm text-foreground-subtlest">
        {intl.formatMessage({ id: "circuit.bom.empty" })}
      </p>
    );
  return (
    <ul data-testid="circuit-bom" className="flex flex-col">
      {components.map((component) => (
        <li key={component.id}>
          <button
            type="button"
            data-testid="circuit-bom-row"
            data-circuit-element-id={component.id}
            aria-pressed={component.id === selectedComponentId}
            onClick={() => onSelect?.(component.id)}
            className={cn(
              "flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-ui-sm hover:bg-hover",
              component.id === selectedComponentId && "bg-selected",
            )}
          >
            <span className="min-w-0 flex-1 truncate font-mono">{componentLabel(component)}</span>
            {component.footprint ? (
              <span className="max-w-32 truncate text-ui-xs text-foreground-subtlest">
                {component.footprint}
              </span>
            ) : null}
            {component.pins.length > 0 ? (
              <span className="shrink-0 text-ui-xs text-foreground-subtlest">
                {component.pins.length}
              </span>
            ) : null}
            {lowConfidenceIds.has(component.id) ? <LowConfidenceBadge /> : null}
          </button>
        </li>
      ))}
    </ul>
  );
});

export const CircuitNetList = memo(function CircuitNetList({
  nets,
  componentsById,
  lowConfidenceIds,
  selectedNetId,
  onSelect,
}: {
  nets: readonly CircuitNet[];
  componentsById: ReadonlyMap<string, CircuitComponent>;
  lowConfidenceIds: ReadonlySet<string>;
  selectedNetId?: string;
  onSelect?: (netId: string) => void;
}) {
  const { intl } = useESCodeIntl();
  if (nets.length === 0)
    return (
      <p className="px-2 py-1 text-ui-sm text-foreground-subtlest">
        {intl.formatMessage({ id: "circuit.nets.empty" })}
      </p>
    );
  return (
    <ul data-testid="circuit-nets" className="flex flex-col">
      {nets.map((net) => (
        <li key={net.id}>
          <button
            type="button"
            data-testid="circuit-net-row"
            data-circuit-element-id={net.id}
            aria-pressed={net.id === selectedNetId}
            onClick={() => onSelect?.(net.id)}
            className={cn(
              "flex w-full flex-col gap-0.5 rounded-md px-2 py-1 text-left text-ui-sm hover:bg-hover",
              net.id === selectedNetId && "bg-selected",
            )}
          >
            <span className="flex w-full items-center gap-2">
              <span className="min-w-0 flex-1 truncate font-mono">{netLabel(net)}</span>
              <span className="shrink-0 text-ui-xs text-foreground-subtlest">
                {intl.formatMessage({ id: "circuit.nets.pinCount" }, { count: net.pinRefs.length })}
              </span>
              {lowConfidenceIds.has(net.id) ? <LowConfidenceBadge /> : null}
            </span>
            {net.id === selectedNetId ? (
              <span className="min-w-0 break-words text-ui-xs text-foreground-subtle">
                {net.pinRefs.length > 0
                  ? net.pinRefs
                      .map((pinRef) => {
                        const component = componentsById.get(pinRef.componentId);
                        return `${component?.ref ?? pinRef.componentId}.${pinLabel({ number: pinRef.pin })}`;
                      })
                      .join(" · ")
                  : intl.formatMessage({ id: "circuit.nets.noPins" })}
              </span>
            ) : null}
          </button>
        </li>
      ))}
    </ul>
  );
});
