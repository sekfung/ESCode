import { useCallback, useEffect, useRef, useState } from "react";
import type { IServiceAccessor, SerialConfig, SerialErrorCode, SerialState } from "@zcode/services";
import type { SerialPortPreferences } from "@zcode/shared";
import { toast } from "@/components/ui/toast.js";
import { useSettings } from "@/hooks/useSettingService.js";
import { useSerialSession } from "@/hooks/useSerialSession.js";
import { useZCodeIntl } from "@/i18n/IntlProvider.js";
import { logger } from "@/logger.js";
import { SerialConnectionBar } from "@/serial/SerialConnectionBar.js";
import { SerialLogView } from "@/serial/SerialLogView.js";
import { SerialSendBar } from "@/serial/SerialSendBar.js";
import { cn } from "@/components/lib/utils.js";

const DEFAULT_CONFIG: SerialConfig = {
  baudRate: 115200,
  dataBits: 8,
  parity: "none",
  stopBits: 1,
  rtscts: false,
  autoReconnect: true,
};

const BUSY_STATES: ReadonlySet<SerialState> = new Set(["opening", "closing"]);

function describeError(error: unknown): { code?: SerialErrorCode; message: string } {
  const code = (error as { code?: unknown } | null)?.code;
  return {
    code: typeof code === "string" ? (code as SerialErrorCode) : undefined,
    message: error instanceof Error ? error.message : String(error),
  };
}

/**
 * 串口调试器面板。串口会话属于窗口级 Desktop Local Host：关闭标签不会断开串口，
 * 重新打开时从 Host 快照恢复收发记录。
 */
export function SerialPane({
  services,
  isVisible,
}: {
  services: IServiceAccessor;
  isVisible: boolean;
}) {
  const { intl } = useZCodeIntl();
  const { settings, update } = useSettings();
  const { state, refreshPorts, open, close, send, clear } = useSerialSession(
    services.serialService,
    isVisible,
  );
  const { status, ports, log, listError } = state;
  const isActive =
    status.state === "open" || status.state === "disconnected" || BUSY_STATES.has(status.state);

  const preferences = settings?.serialPortPreferences;
  const [path, setPath] = useState("");
  const [config, setConfig] = useState<SerialConfig>(DEFAULT_CONFIG);
  const formInitialized = useRef(false);

  // 表单预填：已有会话时以 Host 状态为准；否则使用上次记住的串口与参数（只预填，不自动连接）。
  useEffect(() => {
    if (formInitialized.current) return;
    if (status.path && status.config) {
      formInitialized.current = true;
      setPath(status.path);
      setConfig(status.config);
      return;
    }
    if (!settings) return;
    formInitialized.current = true;
    const lastPath = preferences?.lastPath;
    if (lastPath) {
      setPath(lastPath);
      setConfig(preferences.byPath[lastPath] ?? DEFAULT_CONFIG);
    }
  }, [preferences, settings, status.config, status.path]);

  useEffect(() => {
    if (!path && ports[0]) setPath(ports[0].path);
  }, [path, ports]);

  const selectPort = useCallback(
    (nextPath: string) => {
      setPath(nextPath);
      const remembered = preferences?.byPath[nextPath];
      if (remembered) setConfig(remembered);
    },
    [preferences],
  );

  const formatError = useCallback(
    (error: { code?: string; message: string }) =>
      intl.formatMessage({ id: `serial.error.${error.code ?? "io"}` }, { message: error.message }),
    [intl],
  );

  const handleToggleOpen = useCallback(async () => {
    try {
      if (isActive) {
        await close();
        return;
      }
      await open(path, config);
      const next: SerialPortPreferences = {
        lastPath: path,
        byPath: { ...preferences?.byPath, [path]: config },
      };
      void update({ serialPortPreferences: next }).catch((error: unknown) => {
        logger.warn("[serial] failed to remember port preference", describeError(error));
      });
    } catch (error) {
      // 打开失败的详情已由 onStatus(error) 显示在状态行；这里只记录日志。
      logger.warn("[serial] toggle failed", describeError(error));
    }
  }, [close, config, isActive, open, path, preferences, update]);

  const handleSend = useCallback(
    async (bytes: Uint8Array) => {
      try {
        await send(bytes);
        return true;
      } catch (error) {
        toast(formatError(describeError(error)));
        return false;
      }
    },
    [formatError, send],
  );

  const handleClear = useCallback(() => {
    void clear().catch((error: unknown) => {
      logger.warn("[serial] clear failed", describeError(error));
    });
  }, [clear]);

  if (!services.serialService) {
    return (
      <section className="flex h-full items-center justify-center bg-background p-4 text-ui-sm text-foreground-subtle">
        {intl.formatMessage({ id: "serial.unavailable" })}
      </section>
    );
  }

  const statusText =
    status.state === "error" && status.error
      ? formatError(status.error)
      : intl.formatMessage({ id: `serial.state.${status.state}` });

  return (
    <section
      className="flex h-full min-h-0 flex-col gap-2 bg-background p-3"
      data-testid="serial-pane"
    >
      <SerialConnectionBar
        ports={ports}
        path={path}
        config={config}
        isActive={isActive}
        isBusy={BUSY_STATES.has(status.state)}
        onSelectPort={selectPort}
        onRefreshPorts={() => void refreshPorts()}
        onConfigChange={setConfig}
        onToggleOpen={() => void handleToggleOpen()}
      />
      <div className="flex min-w-0 items-center gap-2 text-ui-sm">
        <span
          className={cn(
            "size-2 shrink-0 rounded-full",
            status.state === "open"
              ? "bg-success"
              : status.state === "disconnected"
                ? "bg-warning"
                : status.state === "error"
                  ? "bg-destructive"
                  : "bg-foreground-subtlest",
          )}
        />
        <span
          className={cn(
            "min-w-0 flex-1 truncate",
            status.state === "error" ? "text-destructive" : "text-foreground-subtle",
          )}
          title={statusText}
          data-testid="serial-status"
          data-state={status.state}
        >
          {statusText}
          {listError ? ` · ${formatError(listError)}` : ""}
        </span>
        <span className="shrink-0 font-mono text-ui-xs text-foreground-subtlest">
          {intl.formatMessage(
            { id: "serial.stats" },
            { rx: log.stats.rxBytes, tx: log.stats.txBytes },
          )}
        </span>
      </div>
      <SerialLogView chunks={log.chunks} onClear={handleClear} />
      <SerialSendBar canSend={status.state === "open"} onSend={handleSend} />
    </section>
  );
}
