import { CircuitBoardIcon, XIcon } from "lucide-react";
import type { AttachmentHoverCardContentProps } from "@/components/ai-elements/attachments.js";
import { Button } from "@/components/ui/button.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import type { CircuitDraftReference } from "@/circuit/domain/circuitReferences.js";
import { ContextAttachmentPill } from "@/v4/composer/ContextAttachmentPill.js";

/**
 * 电路引用 chip（规格 §8）：输入框草稿与历史消息共用同一枚 pill。
 * 插入后不自动发送——chip 只属于 renderer 草稿，发送仍由用户触发。
 */
export function CircuitReferenceChip({
  references,
  contentAlign = "start",
  onRemove,
  onRemoveAll,
}: {
  references: readonly CircuitDraftReference[];
  contentAlign?: AttachmentHoverCardContentProps["align"];
  onRemove?: (reference: CircuitDraftReference) => void;
  onRemoveAll?: () => void;
}) {
  const { intl } = useESCodeIntl();
  if (references.length === 0) return null;
  const label =
    references.length === 1 && references[0]
      ? references[0].label
      : intl.formatMessage({ id: "circuit.reference.count" }, { count: references.length });
  const removeLabel = intl.formatMessage({ id: "circuit.reference.remove" });
  return (
    <ContextAttachmentPill
      contentAlign={contentAlign}
      icon={<CircuitBoardIcon className="size-4 shrink-0 text-foreground-subtle" />}
      label={label}
      onRemoveAll={onRemoveAll}
      removeLabel={removeLabel}
      triggerProps={{
        "data-testid": "circuit-reference-chip",
        "data-circuit-reference-count": references.length,
      }}
    >
      {references.map((reference) => (
        <div
          key={`${reference.irId}:${reference.kind}:${reference.name}`}
          className="group/reference flex items-center gap-2 rounded-lg px-2 py-1.5 text-ui-base hover:bg-menu-hover"
        >
          <CircuitBoardIcon className="size-4 shrink-0 text-foreground-subtle" />
          <div className="min-w-0 flex-1">
            <div className="truncate font-mono" data-testid="circuit-reference-chip-label">
              {reference.label}
            </div>
            <div className="mt-0.5 break-words text-ui-sm text-foreground-subtlest">
              {reference.detail} · {reference.irId}
            </div>
          </div>
          {onRemove ? (
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              className="size-5 shrink-0 text-foreground-subtle opacity-0 group-hover/reference:opacity-100"
              aria-label={removeLabel}
              onClick={() => onRemove(reference)}
            >
              <XIcon className="size-3.5" />
            </Button>
          ) : null}
        </div>
      ))}
    </ContextAttachmentPill>
  );
}
