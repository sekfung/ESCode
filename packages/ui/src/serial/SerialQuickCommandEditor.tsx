import { useState } from "react";
import type { SerialQuickCommand } from "@escode/shared";
import { Button } from "@/components/ui/button.js";
import { Input } from "@/components/ui/input.js";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select.js";
import { Textarea } from "@/components/ui/textarea.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import { buildSerialSendPayload } from "@/lib/serial/serialFormat.js";
import type { SerialQuickCommandDraft } from "@/lib/serial/serialQuickCommands.js";
import { SegmentedToggle } from "@/serial/SerialControls.js";
import { SerialChecksumPicker } from "@/serial/SerialChecksumPicker.js";

const EMPTY_DRAFT: SerialQuickCommandDraft = {
  name: "",
  data: "",
  mode: "text",
  lineEnding: "crlf",
};

/** 快捷指令的新增/编辑表单；HEX 内容用与发送相同的解析规则即时校验。 */
export function SerialQuickCommandEditor({
  initial,
  onSave,
  onCancel,
}: {
  initial?: SerialQuickCommand;
  onSave: (draft: SerialQuickCommandDraft) => void;
  onCancel: () => void;
}) {
  const { intl } = useESCodeIntl();
  const [draft, setDraft] = useState<SerialQuickCommandDraft>(() =>
    initial
      ? {
          name: initial.name,
          data: initial.data,
          mode: initial.mode,
          lineEnding: initial.lineEnding,
          ...(initial.checksum ? { checksum: initial.checksum } : {}),
        }
      : EMPTY_DRAFT,
  );
  const payload = buildSerialSendPayload({
    input: draft.data,
    mode: draft.mode,
    lineEnding: draft.lineEnding,
  });
  const hexError =
    draft.mode === "hex" && !payload.ok
      ? intl.formatMessage({ id: `serial.hexError.${payload.error}` })
      : null;
  const canSave = draft.name.trim().length > 0 && draft.data.length > 0 && !hexError;

  return (
    <form
      className="flex w-72 flex-col gap-2"
      data-testid="serial-quick-command-editor"
      onSubmit={(event) => {
        event.preventDefault();
        if (canSave) onSave({ ...draft, name: draft.name.trim() });
      }}
    >
      <Input
        autoFocus
        value={draft.name}
        maxLength={64}
        placeholder={intl.formatMessage({ id: "serial.quick.name" })}
        onChange={(event) => setDraft((current) => ({ ...current, name: event.target.value }))}
        data-testid="serial-quick-command-name"
      />
      <Textarea
        className="min-h-16 resize-none font-mono"
        value={draft.data}
        placeholder={intl.formatMessage({
          id: draft.mode === "hex" ? "serial.send.hexPlaceholder" : "serial.quick.data",
        })}
        aria-invalid={hexError ? true : undefined}
        onChange={(event) => setDraft((current) => ({ ...current, data: event.target.value }))}
        data-testid="serial-quick-command-data"
      />
      <div className="flex items-center gap-2">
        <SegmentedToggle
          value={draft.mode}
          onChange={(mode) => setDraft((current) => ({ ...current, mode }))}
          options={[
            { value: "text", label: intl.formatMessage({ id: "serial.mode.text" }) },
            { value: "hex", label: "HEX" },
          ]}
        />
        <Select
          value={draft.lineEnding}
          disabled={draft.mode === "hex"}
          onValueChange={(lineEnding) =>
            setDraft((current) => ({
              ...current,
              lineEnding: lineEnding as SerialQuickCommandDraft["lineEnding"],
            }))
          }
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
      </div>
      <SerialChecksumPicker
        value={draft.checksum}
        onChange={(checksum) =>
          setDraft(({ checksum: _previous, ...current }) =>
            checksum ? { ...current, checksum } : current,
          )
        }
        testId="serial-quick-command-checksum"
      />
      {hexError ? <span className="text-ui-sm text-destructive">{hexError}</span> : null}
      <div className="flex justify-end gap-2">
        <Button type="button" variant="ghost" size="sm" onClick={onCancel}>
          {intl.formatMessage({ id: "serial.quick.cancel" })}
        </Button>
        <Button type="submit" size="sm" disabled={!canSave} data-testid="serial-quick-command-save">
          {intl.formatMessage({ id: "serial.quick.save" })}
        </Button>
      </div>
    </form>
  );
}
