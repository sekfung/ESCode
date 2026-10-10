import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ArrowDownToLineIcon, DownloadIcon, EraserIcon, PauseIcon, PlayIcon } from "lucide-react";
import type { SerialChunk } from "@zcode/services";
import { Button } from "@/components/ui/button.js";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select.js";
import { Switch } from "@/components/ui/switch.js";
import { toast } from "@/components/ui/toast.js";
import { usePlatform } from "@/hooks/usePlatform.js";
import { useZCodeIntl } from "@/i18n/IntlProvider.js";
import {
  buildSerialDisplayRows,
  buildSerialExportText,
  type SerialDisplayEncoding,
  type SerialSendMode,
} from "@/lib/serial/serialFormat.js";
import { IconToggle, SegmentedToggle } from "@/serial/SerialControls.js";
import { cn } from "@/components/lib/utils.js";

/** 渲染层显示上限；超出时只截断显示，不影响 Host 缓冲与导出。 */
const MAX_RENDERED_ROWS = 2000;

function formatTime(at: number): string {
  const date = new Date(at);
  const pad = (value: number, length = 2) => String(value).padStart(length, "0");
  return `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}.${pad(date.getMilliseconds(), 3)}`;
}

export function SerialLogView({
  chunks,
  onClear,
  getAgentLabel,
  onOpenAgentSession,
}: {
  chunks: readonly SerialChunk[];
  onClear: () => void;
  /** Agent 写入行的会话标注；查不到标题时由调用方回退为会话 ID 前缀。 */
  getAgentLabel: (sessionId: string | undefined) => string;
  onOpenAgentSession?: (sessionId: string) => void;
}) {
  const { intl } = useZCodeIntl();
  const platform = usePlatform();
  const [displayMode, setDisplayMode] = useState<SerialSendMode>("text");
  const [encoding, setEncoding] = useState<SerialDisplayEncoding>("utf-8");
  const [showTimestamp, setShowTimestamp] = useState(false);
  const [autoScroll, setAutoScroll] = useState(true);
  // 暂停只冻结显示；Host 继续接收并记录数据，恢复后一次性补齐。
  const [pausedChunks, setPausedChunks] = useState<readonly SerialChunk[] | null>(null);
  const visibleChunks = pausedChunks ?? chunks;
  const rows = useMemo(() => {
    const all = buildSerialDisplayRows(visibleChunks, {
      mode: displayMode,
      encoding,
      showTimestamp,
    });
    return all.length > MAX_RENDERED_ROWS ? all.slice(-MAX_RENDERED_ROWS) : all;
  }, [displayMode, encoding, showTimestamp, visibleChunks]);
  const scrollRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!autoScroll || !scrollRef.current) return;
    scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
  }, [autoScroll, rows]);

  const handleClear = useCallback(() => {
    setPausedChunks((current) => (current ? [] : current));
    onClear();
  }, [onClear]);

  const handleExport = useCallback(async () => {
    if (!platform.saveFile) return;
    const data = new TextEncoder().encode(
      buildSerialExportText(chunks, { mode: displayMode, encoding }),
    );
    const stamp = new Date().toISOString().replace(/[:.]/g, "-");
    const reportFailure = (message: string) =>
      toast(intl.formatMessage({ id: "serial.exportFailed" }, { message }));
    try {
      const result = await platform.saveFile({
        data: data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength),
        suggestedName: `serial-${stamp}.log`,
      });
      if (!result.success && !result.canceled) reportFailure(result.error ?? "");
    } catch (error) {
      reportFailure(error instanceof Error ? error.message : String(error));
    }
  }, [chunks, displayMode, encoding, intl, platform]);

  return (
    <>
      <div className="flex flex-wrap items-center gap-1">
        <SegmentedToggle
          value={displayMode}
          onChange={setDisplayMode}
          options={[
            { value: "text", label: intl.formatMessage({ id: "serial.mode.text" }) },
            { value: "hex", label: "HEX" },
          ]}
          testId="serial-display-mode"
        />
        <Select
          value={encoding}
          onValueChange={(value) => setEncoding(value as SerialDisplayEncoding)}
          disabled={displayMode === "hex"}
        >
          <SelectTrigger size="sm" aria-label={intl.formatMessage({ id: "serial.encoding" })}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="utf-8">UTF-8</SelectItem>
            <SelectItem value="gbk">GBK</SelectItem>
          </SelectContent>
        </Select>
        <label className="flex items-center gap-1.5 px-1 text-ui-sm text-foreground-subtle">
          <Switch size="sm" checked={showTimestamp} onCheckedChange={setShowTimestamp} />
          {intl.formatMessage({ id: "serial.timestamp" })}
        </label>
        <div className="ml-auto flex items-center gap-0.5">
          <IconToggle
            active={autoScroll}
            label={intl.formatMessage({ id: "serial.autoScroll" })}
            onClick={() => setAutoScroll((current) => !current)}
          >
            <ArrowDownToLineIcon />
          </IconToggle>
          <IconToggle
            active={pausedChunks !== null}
            label={intl.formatMessage({ id: pausedChunks ? "serial.resume" : "serial.pause" })}
            onClick={() => setPausedChunks((current) => (current ? null : chunks))}
          >
            {pausedChunks ? <PlayIcon /> : <PauseIcon />}
          </IconToggle>
          <Button
            variant="ghost"
            size="icon"
            onClick={handleClear}
            aria-label={intl.formatMessage({ id: "serial.clear" })}
            title={intl.formatMessage({ id: "serial.clear" })}
            data-testid="serial-clear"
          >
            <EraserIcon />
          </Button>
          {platform.saveFile ? (
            <Button
              variant="ghost"
              size="icon"
              onClick={() => void handleExport()}
              aria-label={intl.formatMessage({ id: "serial.export" })}
              title={intl.formatMessage({ id: "serial.export" })}
              data-testid="serial-export"
            >
              <DownloadIcon />
            </Button>
          ) : null}
        </div>
      </div>
      <div
        ref={scrollRef}
        className="min-h-0 flex-1 overflow-auto rounded-xl border border-border bg-surface p-2 font-mono text-ui-sm"
        data-testid="serial-log"
        onWheel={(event) => {
          // 用户向上翻阅时停止自动滚动，避免新数据把视图拉回底部。
          if (event.deltaY < 0 && autoScroll) setAutoScroll(false);
        }}
      >
        {pausedChunks ? (
          <div className="mb-1 text-ui-xs text-warning">
            {intl.formatMessage({ id: "serial.paused" })}
          </div>
        ) : null}
        {rows.map((row) => (
          <div
            key={row.key}
            className={cn(
              "flex gap-2 break-all whitespace-pre-wrap",
              row.direction === "tx" ? "text-icon-blue" : "text-foreground",
            )}
            data-direction={row.direction}
          >
            {showTimestamp ? (
              <span className="shrink-0 text-foreground-subtlest">{formatTime(row.at)}</span>
            ) : null}
            {row.direction === "tx" ? <span className="shrink-0">→</span> : null}
            {row.source === "agent" ? (
              <button
                type="button"
                className="shrink-0 text-foreground-subtle hover:text-foreground hover:underline disabled:no-underline"
                disabled={!row.sessionId || !onOpenAgentSession}
                onClick={() => row.sessionId && onOpenAgentSession?.(row.sessionId)}
                data-testid="serial-agent-label"
              >
                [Agent·{getAgentLabel(row.sessionId)}]
              </button>
            ) : null}
            <span className="min-w-0">{row.text}</span>
          </div>
        ))}
      </div>
    </>
  );
}
