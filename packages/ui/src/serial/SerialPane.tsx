import { useCallback, useEffect, useMemo, useState } from "react";
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
import {
  formatSerialAgentLabel,
  resolveSerialAgentSession,
} from "@/lib/serial/serialAgentSession.js";
import { useZCodeSessionStore } from "@/store/zcodeSessionStore.js";
import { buildSerialPortOptions, pickInitialSerialPath } from "@/lib/serial/serialPortChoice.js";

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
 * 串口调试器面板（一个标签对应一个串口）。串口会话属于窗口级 Desktop Local Host：关闭标签不会断开串口，
 * 选择运行中的串口即挂回该会话，并从 Host 快照恢复收发记录。
 */
export function SerialPane({
  services,
  isVisible,
  path,
  onBindPath,
  onOpenSession,
}: {
  services: IServiceAccessor;
  isVisible: boolean;
  /** 标签绑定的串口路径（标签状态是唯一来源）；未绑定时为空。 */
  path?: string;
  onBindPath: (path: string) => void;
  /** 点击 `[Agent·…]` 时跳到发起写入的会话。 */
  onOpenSession?: (workspacePath: string, taskId: string, workspaceIdentity?: string) => void;
}) {
  const { intl } = useZCodeIntl();
  const { settings, update } = useSettings();
  const { state, refreshPorts, open, close, send, clear } = useSerialSession(
    services.serialService,
    path,
    isVisible,
  );
  const { status, ports, sessions, log, listError } = state;
  const isActive =
    status.state === "open" || status.state === "disconnected" || BUSY_STATES.has(status.state);

  const preferences = settings?.serialPortPreferences;
  const [config, setConfig] = useState<SerialConfig>(DEFAULT_CONFIG);
  const portOptions = useMemo(() => buildSerialPortOptions(ports, sessions), [ports, sessions]);

  // 未绑定的新标签预选串口（只预选，不自动连接）：上次用过且未被占用的优先，否则第一个空闲串口。
  useEffect(() => {
    if (path || !settings || !state.inventoryLoaded || ports.length === 0) return;
    const initial = pickInitialSerialPath({ lastPath: preferences?.lastPath, ports, sessions });
    if (initial) onBindPath(initial);
  }, [onBindPath, path, ports, preferences?.lastPath, sessions, settings, state.inventoryLoaded]);

  // 表单参数：会话活动时以 Host 状态为准；否则用该串口记住的参数。
  useEffect(() => {
    if (status.config && status.state !== "closed") {
      setConfig(status.config);
      return;
    }
    if (path) setConfig(preferences?.byPath[path] ?? DEFAULT_CONFIG);
  }, [path, preferences, status.config, status.state]);

  const selectPort = useCallback((nextPath: string) => onBindPath(nextPath), [onBindPath]);

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
      if (!path) return;
      await open(config);
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

  const workspaces = useZCodeSessionStore((state) => state.workspaces);
  const getAgentLabel = useCallback(
    (sessionId: string | undefined) =>
      sessionId
        ? formatSerialAgentLabel(resolveSerialAgentSession(workspaces, sessionId)?.title, sessionId)
        : "Agent",
    [workspaces],
  );
  const handleOpenAgentSession = useCallback(
    (sessionId: string) => {
      const session = resolveSerialAgentSession(workspaces, sessionId);
      if (session) onOpenSession?.(session.workspacePath, sessionId, session.workspaceIdentity);
    },
    [onOpenSession, workspaces],
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
        ports={portOptions}
        path={path ?? ""}
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
      <SerialLogView
        chunks={log.chunks}
        onClear={handleClear}
        getAgentLabel={getAgentLabel}
        {...(onOpenSession ? { onOpenAgentSession: handleOpenAgentSession } : {})}
      />
      <SerialSendBar canSend={status.state === "open"} onSend={handleSend} />
    </section>
  );
}
