import { memo, useMemo, type ComponentProps } from "react";
import { GenUiMessageResponse } from "@/gen-ui/index.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import { projectCircuitReferences } from "@/circuit/domain/circuitReferences.js";
import { CircuitReferenceCard } from "@/circuit/components/CircuitReferenceCard.js";

/**
 * assistant 正文里同时渲染 `::circuit` 引用卡片（规格 §8）与 Gen UI 引用。
 * 投影规则与 Gen UI 一致（代码块内不识别、消息完成后才渲染）：
 * 先按 `::circuit` 切分正文，文本段再交给 GenUiMessageResponse 处理 `::visualize`，
 * 两种 directive 互不干扰，也不改变各自已有的严格校验。
 */

type GenUiMessageResponseProps = ComponentProps<typeof GenUiMessageResponse>;

export const CircuitMessageResponse = memo(function CircuitMessageResponse({
  completed,
  messageKey,
  children,
  ...props
}: GenUiMessageResponseProps) {
  const { intl } = useESCodeIntl();
  const text = typeof children === "string" ? children : "";
  const parts = useMemo(() => projectCircuitReferences(text, completed), [completed, text]);
  const workspace = {
    ...(props.workspacePath ? { workspacePath: props.workspacePath } : {}),
    ...(props.workspaceIdentity ? { workspaceIdentity: props.workspaceIdentity } : {}),
    ...(props.workspaceRemoteSessionId
      ? { remoteSessionId: props.workspaceRemoteSessionId }
      : {}),
  };
  return parts.map((part, index) =>
    part.kind === "text" ? (
      <GenUiMessageResponse
        key={`text:${index}`}
        completed={completed}
        messageKey={`${messageKey}:text:${index}`}
        {...props}
      >
        {part.text}
      </GenUiMessageResponse>
    ) : part.kind === "circuit" ? (
      <CircuitReferenceCard
        key={`circuit:${part.offset}`}
        reference={part.reference}
        workspace={workspace}
        onOpenCodeViewer={props.onOpenCodeViewer}
      />
    ) : (
      <div
        key={`status:${index}`}
        className="my-3 rounded-xl border border-panel-border p-3 text-ui-sm text-foreground-subtle"
        data-testid={`circuit-card-${part.kind}`}
      >
        {intl.formatMessage({
          id:
            part.kind === "pending"
              ? props.streaming
                ? "circuit.pending"
                : "circuit.incomplete"
              : "circuit.invalid",
        })}
      </div>
    ),
  );
});
