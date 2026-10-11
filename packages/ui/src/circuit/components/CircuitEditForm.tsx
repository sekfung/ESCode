import { useMemo, useState } from "react";
import type { CircuitCorrectionOp } from "@escode/shared/circuit";
import type { CircuitComponent, CircuitNet } from "@escode/shared/circuit";
import { Button } from "@/components/ui/button.js";
import { Input } from "@/components/ui/input.js";
import { Label } from "@/components/ui/label.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import { netLabel, pinLabel } from "@/circuit/domain/circuitView.js";

/**
 * 器件属性与连接的修正表单（规格 §8「改位号、型号、值，增删连接」）。
 * 这里只产出 `CircuitCorrectionOp[]` 交给 hooks 的 correct()：写入必须带 baseRevision、
 * 被 stale_revision 拒绝时不覆盖数据，都由 hook 按契约处理，组件不做兜底合并。
 */

const DISCONNECT_VALUE = "\u0000disconnect";

export function CircuitEditForm({
  component,
  nets,
  disabled,
  onSubmit,
}: {
  component: CircuitComponent;
  nets: readonly CircuitNet[];
  disabled: boolean;
  onSubmit: (ops: readonly CircuitCorrectionOp[]) => void;
}) {
  const { intl } = useESCodeIntl();
  const [ref, setRef] = useState(component.ref);
  const [value, setValue] = useState(component.value ?? "");
  const [partNumber, setPartNumber] = useState(component.partNumber ?? "");
  const [pinNets, setPinNets] = useState<Record<string, string>>(() =>
    Object.fromEntries(
      component.pins.map((pin) => [pin.number, pin.netId ?? DISCONNECT_VALUE]),
    ),
  );
  const netOptions = useMemo(
    () => [...nets].sort((left, right) => netLabel(left).localeCompare(netLabel(right))),
    [nets],
  );

  const ops = useMemo((): CircuitCorrectionOp[] => {
    const result: CircuitCorrectionOp[] = [];
    const trimmedRef = ref.trim();
    const trimmedValue = value.trim();
    const trimmedPartNumber = partNumber.trim();
    if (trimmedRef && trimmedRef !== component.ref)
      result.push({ op: "update-component", componentId: component.id, ref: trimmedRef });
    if (trimmedValue !== (component.value ?? ""))
      result.push({ op: "update-component", componentId: component.id, value: trimmedValue });
    if (trimmedPartNumber !== (component.partNumber ?? ""))
      result.push({
        op: "update-component",
        componentId: component.id,
        partNumber: trimmedPartNumber,
      });
    for (const pin of component.pins) {
      const next = pinNets[pin.number] ?? DISCONNECT_VALUE;
      const current = pin.netId ?? DISCONNECT_VALUE;
      if (next === current) continue;
      if (next === DISCONNECT_VALUE)
        result.push({ op: "disconnect", componentId: component.id, pin: pin.number });
      else result.push({ op: "connect", componentId: component.id, pin: pin.number, netId: next });
    }
    return result;
  }, [component, partNumber, pinNets, ref, value]);

  return (
    <form
      data-testid="circuit-edit-form"
      className="flex flex-col gap-2 px-2 pb-2"
      onSubmit={(event) => {
        event.preventDefault();
        if (disabled || ops.length === 0) return;
        onSubmit(ops);
      }}
    >
      <div className="grid grid-cols-[auto_1fr] items-center gap-x-2 gap-y-1">
        <Label htmlFor="circuit-edit-ref" className="text-ui-sm">
          {intl.formatMessage({ id: "circuit.edit.ref" })}
        </Label>
        <Input
          id="circuit-edit-ref"
          size="sm"
          value={ref}
          disabled={disabled}
          onChange={(event) => setRef(event.target.value)}
          data-testid="circuit-edit-ref"
        />
        <Label htmlFor="circuit-edit-value" className="text-ui-sm">
          {intl.formatMessage({ id: "circuit.edit.value" })}
        </Label>
        <Input
          id="circuit-edit-value"
          size="sm"
          value={value}
          disabled={disabled}
          onChange={(event) => setValue(event.target.value)}
          data-testid="circuit-edit-value"
        />
        <Label htmlFor="circuit-edit-part" className="text-ui-sm">
          {intl.formatMessage({ id: "circuit.edit.partNumber" })}
        </Label>
        <Input
          id="circuit-edit-part"
          size="sm"
          value={partNumber}
          disabled={disabled}
          onChange={(event) => setPartNumber(event.target.value)}
          data-testid="circuit-edit-part-number"
        />
      </div>
      {component.pins.length > 0 ? (
        <div className="flex flex-col gap-1">
          <span className="text-ui-sm text-foreground-subtle">
            {intl.formatMessage({ id: "circuit.edit.connections" })}
          </span>
          {component.pins.map((pin) => (
            <div key={pin.number} className="flex items-center gap-2">
              <span className="w-20 shrink-0 truncate font-mono text-ui-sm">{pinLabel(pin)}</span>
              <select
                className="h-6 min-w-0 flex-1 rounded-md border border-input-border bg-input px-1 text-ui-sm text-foreground"
                value={pinNets[pin.number] ?? DISCONNECT_VALUE}
                disabled={disabled}
                aria-label={intl.formatMessage(
                  { id: "circuit.edit.pinNet" },
                  { pin: pinLabel(pin) },
                )}
                data-testid="circuit-edit-pin-net"
                data-circuit-pin={pin.number}
                onChange={(event) =>
                  setPinNets((current) => ({ ...current, [pin.number]: event.target.value }))
                }
              >
                <option value={DISCONNECT_VALUE}>
                  {intl.formatMessage({ id: "circuit.edit.disconnected" })}
                </option>
                {netOptions.map((net) => (
                  <option key={net.id} value={net.id}>
                    {netLabel(net)}
                  </option>
                ))}
              </select>
            </div>
          ))}
        </div>
      ) : null}
      <div className="flex items-center gap-2">
        <Button
          type="submit"
          size="sm"
          variant="default"
          disabled={disabled || ops.length === 0}
          data-testid="circuit-edit-submit"
        >
          {intl.formatMessage({ id: "circuit.edit.apply" }, { count: ops.length })}
        </Button>
        <span className="text-ui-xs text-foreground-subtlest">
          {intl.formatMessage({ id: "circuit.edit.baseRevisionHint" })}
        </span>
      </div>
    </form>
  );
}
