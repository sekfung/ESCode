import { useEffect, useMemo, useState } from "react";
import { useOptionalServices } from "@/hooks/useServices.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import type { SerialSignals } from "@escode/services";
import {
  buildSerialPermissionPreview,
  describeSerialSignalChange,
} from "@/lib/serial/serialPermissionPreview.js";

/**
 * 审批卡片的目标串口与其当前信号：未指定 path 时按 Host 规则推断（恰好一个活动会话时就是它，
 * 否则不显示具体串口，Host 会要求 Agent 指定）。串口服务不存在（Web/远程）时不显示。
 */
function useSerialTarget(
  enabled: boolean,
  explicitPath: string | undefined,
): { path: string | null; signals?: SerialSignals } {
  const services = useOptionalServices();
  const serialService = services?.serialService;
  const [target, setTarget] = useState<{ path: string | null; signals?: SerialSignals }>({
    path: explicitPath ?? null,
  });
  useEffect(() => {
    if (!enabled || !serialService) return;
    let cancelled = false;
    void (async () => {
      const path =
        explicitPath ??
        (await serialService
          .listSessions()
          .then((sessions) => (sessions.length === 1 ? sessions[0]!.path : null)));
      const signals = path ? (await serialService.getSnapshot({ path })).status.signals : undefined;
      if (!cancelled) setTarget({ path: path ?? null, ...(signals ? { signals } : {}) });
    })().catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [enabled, explicitPath, serialService]);
  return target;
}

/**
 * 串口写类工具的审批预览（docs/specs/serial-agent-tools.md）：目标串口、字节数、转义文本与 HEX。
 * 其它工具返回 null，审批卡片保持原样。
 */
export function SerialPermissionPreview({ toolName, input }: { toolName: string; input: unknown }) {
  const { intl } = useESCodeIntl();
  const preview = useMemo(() => buildSerialPermissionPreview(toolName, input), [input, toolName]);
  const needsTarget =
    preview?.kind === "write" || preview?.kind === "close" || preview?.kind === "signals";
  const target = useSerialTarget(
    needsTarget,
    preview && "path" in preview ? preview.path : undefined,
  );
  if (!preview) return null;
  if (preview.kind === "invalid") {
    return (
      <p className="text-ui-sm text-destructive">
        {intl.formatMessage({ id: "serial.permission.invalid" })}
      </p>
    );
  }
  const path = preview.path ?? target.path;
  return (
    <div className="flex flex-col gap-1.5 text-ui-sm" data-testid="serial-permission-preview">
      <div className="flex flex-wrap items-center gap-2 text-foreground-subtle">
        <span>
          {path
            ? intl.formatMessage({ id: "serial.permission.target" }, { path })
            : intl.formatMessage({ id: "serial.permission.noPort" })}
        </span>
        {preview.kind === "open" ? <span className="font-mono">{preview.params}</span> : null}
        {preview.kind === "write" ? (
          <span>
            {intl.formatMessage({ id: "serial.permission.bytes" }, { bytes: preview.bytes })}
          </span>
        ) : null}
      </div>
      {preview.kind === "signals" ? (
        <div className="font-mono text-foreground">
          {preview.pulse
            ? intl.formatMessage({ id: `serial.permission.pulse.${preview.pulse}` })
            : describeSerialSignalChange(preview, target.signals).join("  ")}
        </div>
      ) : null}
      {preview.kind === "write" ? (
        <div className="flex flex-col gap-1 rounded-lg border border-border bg-surface p-2 font-mono text-ui-sm">
          <span className="break-all whitespace-pre-wrap text-foreground">{preview.text}</span>
          <span className="break-all text-foreground-subtle">{preview.hex}</span>
          {preview.truncated ? (
            <span className="text-ui-xs text-foreground-subtlest">
              {intl.formatMessage({ id: "serial.permission.truncated" })}
            </span>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
