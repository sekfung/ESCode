import { useState } from "react";
import { RefreshCwIcon } from "lucide-react";
import type { SerialConfig } from "@zcode/services";
import type { SerialPortOption } from "@/lib/serial/serialPortChoice.js";
import { Button } from "@/components/ui/button.js";
import { Input } from "@/components/ui/input.js";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select.js";
import { useZCodeIntl } from "@/i18n/IntlProvider.js";
import { SerialConfigPopover } from "@/serial/SerialControls.js";

const BAUD_RATE_PRESETS = [
  1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600, 1500000,
];
const CUSTOM_BAUD = "custom";

export function SerialConnectionBar({
  ports,
  path,
  config,
  isActive,
  isBusy,
  onSelectPort,
  onRefreshPorts,
  onConfigChange,
  onToggleOpen,
}: {
  ports: readonly SerialPortOption[];
  path: string;
  config: SerialConfig;
  /** 串口处于打开/等待重连/转换中，参数不可修改。 */
  isActive: boolean;
  isBusy: boolean;
  onSelectPort: (path: string) => void;
  onRefreshPorts: () => void;
  onConfigChange: (updater: (current: SerialConfig) => SerialConfig) => void;
  onToggleOpen: () => void;
}) {
  const { intl } = useZCodeIntl();
  const [customBaud, setCustomBaud] = useState(false);
  const baudValue =
    customBaud || !BAUD_RATE_PRESETS.includes(config.baudRate)
      ? CUSTOM_BAUD
      : String(config.baudRate);

  return (
    <div className="flex flex-wrap items-center gap-2">
      <Select value={path} onValueChange={onSelectPort} disabled={isActive}>
        <SelectTrigger
          className="min-w-0 flex-1 basis-32 font-mono"
          aria-label={intl.formatMessage({ id: "serial.port" })}
          data-testid="serial-port-select"
        >
          <SelectValue placeholder={intl.formatMessage({ id: "serial.port.empty" })} />
        </SelectTrigger>
        <SelectContent>
          {ports.map((port) => (
            <SelectItem key={port.path} value={port.path} className="font-mono">
              {port.path}
              {port.manufacturer ? (
                <span className="ml-2 text-foreground-subtle">{port.manufacturer}</span>
              ) : null}
              {port.running ? (
                <span className="ml-2 text-ui-xs text-success">
                  {intl.formatMessage({ id: "serial.port.running" })}
                </span>
              ) : null}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      <Button
        variant="ghost"
        size="icon"
        onClick={onRefreshPorts}
        aria-label={intl.formatMessage({ id: "serial.refresh" })}
        title={intl.formatMessage({ id: "serial.refresh" })}
      >
        <RefreshCwIcon />
      </Button>
      <Select
        value={baudValue}
        disabled={isActive}
        onValueChange={(value) => {
          if (value === CUSTOM_BAUD) {
            setCustomBaud(true);
            return;
          }
          setCustomBaud(false);
          onConfigChange((current) => ({ ...current, baudRate: Number(value) }));
        }}
      >
        <SelectTrigger
          className="w-28 font-mono"
          aria-label={intl.formatMessage({ id: "serial.baudRate" })}
        >
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {BAUD_RATE_PRESETS.map((rate) => (
            <SelectItem key={rate} value={String(rate)} className="font-mono">
              {rate}
            </SelectItem>
          ))}
          <SelectItem value={CUSTOM_BAUD}>
            {intl.formatMessage({ id: "serial.baudRate.custom" })}
          </SelectItem>
        </SelectContent>
      </Select>
      {baudValue === CUSTOM_BAUD ? (
        <Input
          className="w-24 font-mono"
          inputMode="numeric"
          disabled={isActive}
          value={config.baudRate > 0 ? String(config.baudRate) : ""}
          aria-label={intl.formatMessage({ id: "serial.baudRate" })}
          onChange={(event) => {
            const value = Number.parseInt(event.target.value.replace(/\D/g, ""), 10);
            onConfigChange((current) => ({
              ...current,
              baudRate: Number.isFinite(value) ? value : 0,
            }));
          }}
        />
      ) : null}
      <SerialConfigPopover config={config} disabled={isActive} onChange={onConfigChange} />
      <Button
        variant={isActive ? "outline" : "default"}
        disabled={isBusy || (!isActive && (!path || config.baudRate <= 0))}
        onClick={onToggleOpen}
        data-testid="serial-toggle-open"
      >
        {intl.formatMessage({ id: isActive ? "serial.close" : "serial.open" })}
      </Button>
    </div>
  );
}
