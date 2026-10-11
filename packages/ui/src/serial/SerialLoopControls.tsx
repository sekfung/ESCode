import { Input } from "@/components/ui/input.js";
import { Switch } from "@/components/ui/switch.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import { parseSerialLoopInputs } from "@/lib/serial/serialLoopInputs.js";

export interface SerialLoopSettings {
  enabled: boolean;
  interval: string;
  count: string;
}

export const DEFAULT_SERIAL_LOOP_SETTINGS: SerialLoopSettings = {
  enabled: false,
  interval: "1000",
  count: "",
};

/** 循环发送参数（docs/specs/serial-port-debugger-phase3.md 第 4 节）：间隔 ≥10ms，次数留空为无限。 */
export function SerialLoopControls({
  settings,
  disabled,
  onChange,
}: {
  settings: SerialLoopSettings;
  disabled: boolean;
  onChange: (settings: SerialLoopSettings) => void;
}) {
  const { intl } = useESCodeIntl();
  const parsed = parseSerialLoopInputs(settings.interval, settings.count);
  return (
    <div className="flex flex-wrap items-center gap-1.5 text-ui-sm text-foreground-subtle">
      <label className="flex items-center gap-1.5">
        <Switch
          size="sm"
          checked={settings.enabled}
          disabled={disabled}
          onCheckedChange={(enabled) => onChange({ ...settings, enabled })}
          data-testid="serial-loop-enabled"
        />
        {intl.formatMessage({ id: "serial.loop.title" })}
      </label>
      {settings.enabled ? (
        <>
          <Input
            className="h-6 w-20 font-mono"
            inputMode="numeric"
            value={settings.interval}
            disabled={disabled}
            aria-label={intl.formatMessage({ id: "serial.loop.interval" })}
            aria-invalid={!parsed.ok && parsed.error === "interval" ? true : undefined}
            onChange={(event) => onChange({ ...settings, interval: event.target.value })}
            data-testid="serial-loop-interval"
          />
          <span>ms</span>
          <Input
            className="h-6 w-16 font-mono"
            inputMode="numeric"
            value={settings.count}
            disabled={disabled}
            placeholder="∞"
            aria-label={intl.formatMessage({ id: "serial.loop.count" })}
            aria-invalid={!parsed.ok && parsed.error === "count" ? true : undefined}
            onChange={(event) => onChange({ ...settings, count: event.target.value })}
            data-testid="serial-loop-count"
          />
          <span>{intl.formatMessage({ id: "serial.loop.times" })}</span>
          {!parsed.ok ? (
            <span className="text-destructive">
              {intl.formatMessage({ id: `serial.loop.invalid.${parsed.error}` })}
            </span>
          ) : null}
        </>
      ) : null}
    </div>
  );
}
