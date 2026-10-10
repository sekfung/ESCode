import { Rows3Icon } from "lucide-react";
import type { SerialChecksumConfig } from "@zcode/shared/serial";
import { Button } from "@/components/ui/button.js";
import { Input } from "@/components/ui/input.js";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover.js";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select.js";
import { Switch } from "@/components/ui/switch.js";
import { useZCodeIntl } from "@/i18n/IntlProvider.js";
import {
  DEFAULT_SERIAL_FRAMING_INPUTS,
  parseSerialFramingInputs,
  type SerialFramingInputs,
  type SerialFramingMode,
} from "@/lib/serial/serialFraming.js";
import { SerialChecksumPicker } from "@/serial/SerialChecksumPicker.js";
import { SegmentedToggle } from "@/serial/SerialControls.js";
import { cn } from "@/components/lib/utils.js";

export interface SerialFramingSettings {
  enabled: boolean;
  inputs: SerialFramingInputs;
  /** 帧校验；undefined 表示不校验。 */
  verify?: SerialChecksumConfig;
}

export const DEFAULT_SERIAL_FRAMING_SETTINGS: SerialFramingSettings = {
  enabled: false,
  inputs: DEFAULT_SERIAL_FRAMING_INPUTS,
};

/** 接收分帧设置（docs/specs/serial-port-debugger-phase3.md 第 5 节）；只影响显示，不影响缓冲与导出。 */
export function SerialFramingPopover({
  settings,
  onChange,
}: {
  settings: SerialFramingSettings;
  onChange: (settings: SerialFramingSettings) => void;
}) {
  const { intl } = useZCodeIntl();
  const { inputs } = settings;
  const parsed = parseSerialFramingInputs(inputs);
  const setInputs = (patch: Partial<SerialFramingInputs>) =>
    onChange({ ...settings, inputs: { ...inputs, ...patch } });
  const numeric = (key: "length" | "gapMs", unit: string) => (
    <div className="flex items-center gap-1.5">
      <Input
        className="h-6 w-20 font-mono"
        inputMode="numeric"
        value={inputs[key]}
        aria-label={intl.formatMessage({ id: `serial.framing.${key}` })}
        aria-invalid={!parsed.ok ? true : undefined}
        onChange={(event) => setInputs({ [key]: event.target.value })}
        data-testid={`serial-framing-${key}`}
      />
      <span>{unit}</span>
    </div>
  );

  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button
          variant="ghost"
          size="icon"
          aria-label={intl.formatMessage({ id: "serial.framing.title" })}
          aria-pressed={settings.enabled}
          title={intl.formatMessage({ id: "serial.framing.title" })}
          className={cn(settings.enabled && "bg-selected")}
          data-testid="serial-framing"
        >
          <Rows3Icon />
        </Button>
      </PopoverTrigger>
      <PopoverContent
        align="end"
        className="flex w-72 flex-col gap-2 text-ui-sm text-foreground-subtle"
      >
        <label className="flex items-center gap-1.5 text-foreground">
          <Switch
            size="sm"
            checked={settings.enabled}
            onCheckedChange={(enabled) => onChange({ ...settings, enabled })}
            data-testid="serial-framing-enabled"
          />
          {intl.formatMessage({ id: "serial.framing.enable" })}
        </label>
        <Select
          value={inputs.mode}
          onValueChange={(mode) => setInputs({ mode: mode as SerialFramingMode })}
        >
          <SelectTrigger size="sm" aria-label={intl.formatMessage({ id: "serial.framing.mode" })}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {(["delimiter", "length", "gap"] as const).map((mode) => (
              <SelectItem key={mode} value={mode}>
                {intl.formatMessage({ id: `serial.framing.mode.${mode}` })}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {inputs.mode === "delimiter" ? (
          <div className="flex items-center gap-1.5">
            <SegmentedToggle
              value={inputs.delimiterMode}
              onChange={(delimiterMode) => setInputs({ delimiterMode })}
              options={[
                { value: "text", label: intl.formatMessage({ id: "serial.mode.text" }) },
                { value: "hex", label: "HEX" },
              ]}
            />
            <Input
              className="h-6 flex-1 font-mono"
              value={inputs.delimiter}
              placeholder={inputs.delimiterMode === "hex" ? "0D 0A" : "\\r\\n"}
              aria-label={intl.formatMessage({ id: "serial.framing.delimiter" })}
              aria-invalid={!parsed.ok ? true : undefined}
              onChange={(event) => setInputs({ delimiter: event.target.value })}
              data-testid="serial-framing-delimiter"
            />
          </div>
        ) : inputs.mode === "length" ? (
          numeric("length", intl.formatMessage({ id: "serial.framing.bytes" }))
        ) : (
          numeric("gapMs", "ms")
        )}
        {!parsed.ok ? (
          <span className="text-destructive">
            {intl.formatMessage({ id: `serial.framing.invalid.${parsed.error}` })}
          </span>
        ) : null}
        <span className="text-foreground">
          {intl.formatMessage({ id: "serial.framing.verify" })}
        </span>
        <SerialChecksumPicker
          value={settings.verify}
          onChange={(verify) => {
            const { verify: _previous, ...rest } = settings;
            onChange(verify ? { ...rest, verify } : rest);
          }}
          testId="serial-framing-verify"
        />
      </PopoverContent>
    </Popover>
  );
}
