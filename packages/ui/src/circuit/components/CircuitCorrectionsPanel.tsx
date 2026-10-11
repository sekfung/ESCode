import type { CircuitCorrectionListEntry } from "@escode/services";
import type { CircuitCorrectionOp } from "@escode/shared/circuit";
import { Button } from "@/components/ui/button.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import { cn } from "@/components/lib/utils.js";

/**
 * 修正记录列表（规格 §8 / §3.4）：author、时间、目标元素与修正内容可见；
 * 重新解析后套用不上的记录（orphaned）列在这里，由用户确认或删除（dropCorrection）。
 * 未失效的记录不允许删除——那是服务端的口径，面板不提供入口。
 */

export function describeCorrectionOp(
  op: CircuitCorrectionOp,
  formatMessage: (descriptor: { id: string }, values?: Record<string, string | number>) => string,
): string {
  switch (op.op) {
    case "update-component":
      return formatMessage(
        { id: "circuit.corrections.op.updateComponent" },
        {
          fields: [op.ref, op.value, op.partNumber, op.footprint].filter(Boolean).join(", "),
        },
      );
    case "connect":
      return formatMessage(
        { id: "circuit.corrections.op.connect" },
        { pin: op.pin, net: op.netName ?? op.netId ?? "" },
      );
    case "disconnect":
      return formatMessage({ id: "circuit.corrections.op.disconnect" }, { pin: op.pin });
    case "add-block":
      return formatMessage({ id: "circuit.corrections.op.addBlock" }, { name: op.name });
    case "update-block":
      return formatMessage({ id: "circuit.corrections.op.updateBlock" }, { name: op.blockId });
    case "remove-block":
      return formatMessage({ id: "circuit.corrections.op.removeBlock" }, { name: op.blockId });
    default:
      // 修正记录里的 op 由服务端写入，理论上不会出现未知类型；保留分支让返回值类型穷尽。
      return "";
  }
}

export function CircuitCorrectionsPanel({
  corrections,
  readOnly,
  busy,
  onDrop,
}: {
  corrections: readonly CircuitCorrectionListEntry[];
  readOnly: boolean;
  busy: boolean;
  onDrop: (index: number) => void;
}) {
  const { intl, locale } = useESCodeIntl();
  if (corrections.length === 0)
    return (
      <p className="px-2 py-1 text-ui-sm text-foreground-subtlest">
        {intl.formatMessage({ id: "circuit.corrections.empty" })}
      </p>
    );
  return (
    <ul data-testid="circuit-corrections" className="flex flex-col gap-1 px-1">
      {corrections.map(({ index, record }) => (
        <li
          key={`${index}:${record.revision}`}
          data-testid="circuit-correction-row"
          data-circuit-orphaned={record.orphaned ? "true" : undefined}
          className={cn(
            "flex flex-col gap-0.5 rounded-md px-2 py-1 text-ui-sm",
            record.orphaned ? "bg-warning/10" : "bg-surface/60",
          )}
        >
          <span className="flex items-center gap-2">
            <span className="font-mono text-ui-xs text-foreground-subtlest">r{record.revision}</span>
            <span className="min-w-0 flex-1 truncate">
              {describeCorrectionOp(record.op, intl.formatMessage)}
            </span>
            {record.orphaned ? (
              <span
                className="shrink-0 rounded-sm bg-warning/20 px-1 text-ui-xs text-warning"
                data-testid="circuit-correction-orphaned"
              >
                {intl.formatMessage({ id: "circuit.corrections.orphaned" })}
              </span>
            ) : null}
            {record.orphaned && !readOnly ? (
              <Button
                type="button"
                size="xs"
                variant="ghost"
                disabled={busy}
                onClick={() => onDrop(index)}
                data-testid="circuit-correction-drop"
              >
                {intl.formatMessage({ id: "circuit.corrections.drop" })}
              </Button>
            ) : null}
          </span>
          <span className="text-ui-xs text-foreground-subtlest">
            {intl.formatMessage(
              { id: `circuit.author.${record.author.kind}` },
              { session: record.author.sessionId ?? "" },
            )}{" "}
            ·{" "}
            {new Date(record.appliedAt).toLocaleTimeString(locale, {
              hour: "2-digit",
              minute: "2-digit",
            })}{" "}
            · {record.target.id}
          </span>
        </li>
      ))}
    </ul>
  );
}
