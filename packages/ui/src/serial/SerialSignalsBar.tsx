import { ChevronDownIcon } from "lucide-react";
import type { SerialSignalPulse, SerialSignals } from "@zcode/services";
import { Button } from "@/components/ui/button.js";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu.js";
import { Switch } from "@/components/ui/switch.js";
import { useZCodeIntl } from "@/i18n/IntlProvider.js";

/**
 * DTR/RTS 手动控制与复位脉冲（docs/specs/serial-port-debugger-phase3.md 第 2 节）。
 * 状态由 Host 会话持有，这里只显示与下发；开启 RTS/CTS 流控时 RTS 由硬件控制，开关禁用。
 */
export function SerialSignalsBar({
  signals,
  enabled,
  rtsLocked,
  onChange,
  onPulse,
}: {
  signals: SerialSignals | undefined;
  /** 串口处于 open 时可用。 */
  enabled: boolean;
  rtsLocked: boolean;
  onChange: (patch: { dtr?: boolean; rts?: boolean }) => void;
  onPulse: (pulse: SerialSignalPulse) => void;
}) {
  const { intl } = useZCodeIntl();
  const current = signals ?? { dtr: true, rts: true };
  return (
    <div className="flex flex-wrap items-center gap-3 text-ui-sm text-foreground-subtle">
      <label className="flex items-center gap-1.5 font-mono">
        <Switch
          size="sm"
          checked={current.dtr}
          disabled={!enabled}
          onCheckedChange={(checked) => onChange({ dtr: checked })}
          data-testid="serial-signal-dtr"
        />
        DTR
      </label>
      <label
        className="flex items-center gap-1.5 font-mono"
        title={rtsLocked ? intl.formatMessage({ id: "serial.signals.rtsLocked" }) : undefined}
      >
        <Switch
          size="sm"
          checked={current.rts}
          disabled={!enabled || rtsLocked}
          onCheckedChange={(checked) => onChange({ rts: checked })}
          data-testid="serial-signal-rts"
        />
        RTS
      </label>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="outline" size="sm" disabled={!enabled} data-testid="serial-reset">
            {intl.formatMessage({ id: "serial.signals.reset" })}
            <ChevronDownIcon />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start">
          {(["esp32", "arduino"] as const).map((pulse) => (
            <DropdownMenuItem
              key={pulse}
              onSelect={() => onPulse(pulse)}
              data-testid={`serial-reset-${pulse}`}
            >
              {intl.formatMessage({ id: `serial.signals.reset.${pulse}` })}
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}
