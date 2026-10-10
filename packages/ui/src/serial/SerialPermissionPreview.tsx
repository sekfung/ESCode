import { useEffect, useMemo, useState } from "react";
import { useOptionalServices } from "@/hooks/useServices.js";
import { useZCodeIntl } from "@/i18n/IntlProvider.js";
import { buildSerialPermissionPreview } from "@/lib/serial/serialPermissionPreview.js";

/** 读取当前串口路径用于 write/close 审批；串口服务不存在（Web/远程）时不显示。 */
function useCurrentSerialPath(enabled: boolean): string | null {
  const services = useOptionalServices();
  const serialService = services?.serialService;
  const [path, setPath] = useState<string | null>(null);
  useEffect(() => {
    if (!enabled || !serialService) return;
    let cancelled = false;
    void serialService
      .getSnapshot()
      .then((snapshot) => {
        if (!cancelled) setPath(snapshot.status.path ?? null);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [enabled, serialService]);
  return path;
}

/**
 * 串口写类工具的审批预览（docs/specs/serial-agent-tools.md）：目标串口、字节数、转义文本与 HEX。
 * 其它工具返回 null，审批卡片保持原样。
 */
export function SerialPermissionPreview({ toolName, input }: { toolName: string; input: unknown }) {
  const { intl } = useZCodeIntl();
  const preview = useMemo(() => buildSerialPermissionPreview(toolName, input), [input, toolName]);
  const currentPath = useCurrentSerialPath(preview?.kind === "write" || preview?.kind === "close");
  if (!preview) return null;
  if (preview.kind === "invalid") {
    return (
      <p className="text-ui-sm text-destructive">
        {intl.formatMessage({ id: "serial.permission.invalid" })}
      </p>
    );
  }
  const path = preview.kind === "open" ? preview.path : currentPath;
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
