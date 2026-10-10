import { useCallback, useMemo, useState, type KeyboardEvent } from "react";
import { HistoryIcon, RepeatIcon, SendHorizontalIcon, SquareIcon } from "lucide-react";
import { Button } from "@/components/ui/button.js";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu.js";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select.js";
import { Textarea } from "@/components/ui/textarea.js";
import { useZCodeIntl } from "@/i18n/IntlProvider.js";
import {
  buildSerialSendPayload,
  type SerialLineEnding,
  type SerialSendMode,
} from "@/lib/serial/serialFormat.js";
import { SegmentedToggle } from "@/serial/SerialControls.js";
import { SerialLoopControls, type SerialLoopSettings } from "@/serial/SerialLoopControls.js";
import { parseSerialLoopInputs } from "@/lib/serial/serialLoopInputs.js";

const MAX_SEND_HISTORY = 20;

export function SerialSendBar({
  canSend,
  onSend,
  loop,
}: {
  canSend: boolean;
  /** 发送成功时 resolve；失败由调用方提示，草稿保留以便重试。 */
  onSend: (bytes: Uint8Array) => Promise<boolean>;
  /** 循环发送：参数由面板持有（快捷指令也复用同一份参数），任务由 Host 调度。 */
  loop: {
    settings: SerialLoopSettings;
    running: boolean;
    onSettingsChange: (settings: SerialLoopSettings) => void;
    onStart: (bytes: Uint8Array) => void;
    onStop: () => void;
  };
}) {
  const { intl } = useZCodeIntl();
  const [sendMode, setSendMode] = useState<SerialSendMode>("text");
  const [lineEnding, setLineEnding] = useState<SerialLineEnding>("crlf");
  const [draft, setDraft] = useState("");
  const [history, setHistory] = useState<string[]>([]);
  const payload = useMemo(
    () => buildSerialSendPayload({ input: draft, mode: sendMode, lineEnding }),
    [draft, lineEnding, sendMode],
  );
  const draftError =
    draft.trim() && !payload.ok
      ? intl.formatMessage({ id: `serial.hexError.${payload.error}` })
      : null;
  const sendable = canSend && payload.ok && payload.bytes.byteLength > 0;
  const loopInputs = parseSerialLoopInputs(loop.settings.interval, loop.settings.count);
  const loopMode = loop.settings.enabled || loop.running;

  const handleSend = useCallback(async () => {
    if (loopMode) {
      if (loop.running) loop.onStop();
      else if (sendable && payload.ok && loopInputs.ok) loop.onStart(payload.bytes);
      return;
    }
    if (!sendable || !payload.ok) return;
    const sent = draft;
    if (!(await onSend(payload.bytes))) return;
    setHistory((current) =>
      [sent, ...current.filter((item) => item !== sent)].slice(0, MAX_SEND_HISTORY),
    );
  }, [draft, loop, loopInputs.ok, loopMode, onSend, payload, sendable]);

  const handleKeyDown = useCallback(
    (event: KeyboardEvent<HTMLTextAreaElement>) => {
      if (event.key !== "Enter" || event.shiftKey || event.nativeEvent.isComposing) return;
      event.preventDefault();
      void handleSend();
    },
    [handleSend],
  );

  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex flex-wrap items-center gap-1">
        <SegmentedToggle
          value={sendMode}
          onChange={setSendMode}
          options={[
            { value: "text", label: intl.formatMessage({ id: "serial.mode.text" }) },
            { value: "hex", label: "HEX" },
          ]}
          testId="serial-send-mode"
        />
        <Select
          value={lineEnding}
          onValueChange={(value) => setLineEnding(value as SerialLineEnding)}
          disabled={sendMode === "hex"}
        >
          <SelectTrigger size="sm" aria-label={intl.formatMessage({ id: "serial.lineEnding" })}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {(["none", "cr", "lf", "crlf"] as const).map((value) => (
              <SelectItem key={value} value={value}>
                {intl.formatMessage({ id: `serial.lineEnding.${value}` })}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label={intl.formatMessage({ id: "serial.history" })}
              title={intl.formatMessage({ id: "serial.history" })}
            >
              <HistoryIcon />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" className="max-w-80">
            {history.length === 0 ? (
              <DropdownMenuItem disabled>
                {intl.formatMessage({ id: "serial.history.empty" })}
              </DropdownMenuItem>
            ) : (
              history.map((item) => (
                <DropdownMenuItem key={item} className="font-mono" onSelect={() => setDraft(item)}>
                  <span className="truncate">{item}</span>
                </DropdownMenuItem>
              ))
            )}
          </DropdownMenuContent>
        </DropdownMenu>
        {draftError ? <span className="text-ui-sm text-destructive">{draftError}</span> : null}
      </div>
      <SerialLoopControls
        settings={loop.settings}
        disabled={loop.running}
        onChange={loop.onSettingsChange}
      />
      <div className="flex items-end gap-2">
        <Textarea
          className="min-h-16 flex-1 resize-none font-mono"
          value={draft}
          placeholder={intl.formatMessage({
            id: sendMode === "hex" ? "serial.send.hexPlaceholder" : "serial.send.placeholder",
          })}
          aria-invalid={draftError ? true : undefined}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={handleKeyDown}
          data-testid="serial-send-input"
        />
        <Button
          variant={loop.running ? "outline" : "default"}
          disabled={loop.running ? false : !sendable || (loopMode && !loopInputs.ok)}
          onClick={() => void handleSend()}
          data-testid="serial-send"
        >
          {loop.running ? <SquareIcon /> : loopMode ? <RepeatIcon /> : <SendHorizontalIcon />}
          {intl.formatMessage({
            id: loop.running ? "serial.loop.stop" : loopMode ? "serial.loop.start" : "serial.send",
          })}
        </Button>
      </div>
    </div>
  );
}
