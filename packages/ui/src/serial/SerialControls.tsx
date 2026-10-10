import type { ReactNode } from "react";
import { Settings2Icon } from "lucide-react";
import type { SerialConfig } from "@zcode/services";
import { Button } from "@/components/ui/button.js";
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
import { cn } from "@/components/lib/utils.js";

export function SegmentedToggle<T extends string>({
  value,
  onChange,
  options,
  testId,
}: {
  value: T;
  onChange: (value: T) => void;
  options: ReadonlyArray<{ value: T; label: string }>;
  testId?: string;
}) {
  return (
    <div className="flex items-center rounded-lg border border-border p-0.5" data-testid={testId}>
      {options.map((option) => (
        <button
          key={option.value}
          type="button"
          aria-pressed={value === option.value}
          data-value={option.value}
          className={cn(
            "h-5 rounded-md px-2 text-ui-sm transition-colors",
            value === option.value
              ? "bg-selected text-foreground"
              : "text-foreground-subtle hover:bg-hover",
          )}
          onClick={() => onChange(option.value)}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}

export function IconToggle({
  active,
  label,
  onClick,
  children,
}: {
  active: boolean;
  label: string;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <Button
      variant="ghost"
      size="icon"
      aria-label={label}
      aria-pressed={active}
      title={label}
      className={cn(active && "bg-selected")}
      onClick={onClick}
    >
      {children}
    </Button>
  );
}

export function SerialConfigPopover({
  config,
  disabled,
  onChange,
}: {
  config: SerialConfig;
  disabled: boolean;
  onChange: (updater: (current: SerialConfig) => SerialConfig) => void;
}) {
  const { intl } = useZCodeIntl();
  const field = (labelId: string, control: ReactNode) => (
    <label className="flex items-center justify-between gap-3 text-ui-sm text-foreground-subtle">
      {intl.formatMessage({ id: labelId })}
      {control}
    </label>
  );
  const select = <K extends keyof SerialConfig>(
    key: K,
    values: ReadonlyArray<{ value: SerialConfig[K]; label: string }>,
  ) => (
    <Select
      value={String(config[key])}
      disabled={disabled}
      onValueChange={(raw) => {
        const match = values.find((item) => String(item.value) === raw);
        if (match) onChange((current) => ({ ...current, [key]: match.value }));
      }}
    >
      <SelectTrigger size="sm" className="w-28">
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        {values.map((item) => (
          <SelectItem key={String(item.value)} value={String(item.value)}>
            {item.label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );

  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button
          variant="ghost"
          size="icon"
          aria-label={intl.formatMessage({ id: "serial.settings" })}
          title={intl.formatMessage({ id: "serial.settings" })}
        >
          <Settings2Icon />
        </Button>
      </PopoverTrigger>
      <PopoverContent align="end" className="flex w-64 flex-col gap-2">
        {field(
          "serial.dataBits",
          select(
            "dataBits",
            ([5, 6, 7, 8] as const).map((value) => ({ value, label: String(value) })),
          ),
        )}
        {field(
          "serial.parity",
          select(
            "parity",
            (["none", "even", "odd", "mark", "space"] as const).map((value) => ({
              value,
              label: intl.formatMessage({ id: `serial.parity.${value}` }),
            })),
          ),
        )}
        {field(
          "serial.stopBits",
          select(
            "stopBits",
            ([1, 1.5, 2] as const).map((value) => ({ value, label: String(value) })),
          ),
        )}
        {field(
          "serial.flowControl",
          select("rtscts", [
            { value: false, label: intl.formatMessage({ id: "serial.flowControl.none" }) },
            { value: true, label: "RTS/CTS" },
          ]),
        )}
        {field(
          "serial.autoReconnect",
          <Switch
            size="sm"
            checked={config.autoReconnect}
            disabled={disabled}
            onCheckedChange={(checked) =>
              onChange((current) => ({ ...current, autoReconnect: checked }))
            }
          />,
        )}
      </PopoverContent>
    </Popover>
  );
}
