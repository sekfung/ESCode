import { useCallback, useMemo, useState, type KeyboardEvent } from "react";
import { HistoryIcon, SendHorizontalIcon } from "lucide-react";
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

const MAX_SEND_HISTORY = 20;

export function SerialSendBar({
  canSend,
  onSend,
}: {
  canSend: boolean;
  /** 发送成功时 resolve；失败由调用方提示，草稿保留以便重试。 */
  onSend: (bytes: Uint8Array) => Promise<boolean>;
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

  const handleSend = useCallback(async () => {
    if (!sendable || !payload.ok) return;
    const sent = draft;
    if (!(await onSend(payload.bytes))) return;
    setHistory((current) =>
      [sent, ...current.filter((item) => item !== sent)].slice(0, MAX_SEND_HISTORY),
    );
  }, [draft, onSend, payload, sendable]);

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
        <Button disabled={!sendable} onClick={() => void handleSend()} data-testid="serial-send">
          <SendHorizontalIcon />
          {intl.formatMessage({ id: "serial.send" })}
        </Button>
      </div>
    </div>
  );
}
