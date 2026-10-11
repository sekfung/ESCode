import { useCallback, useEffect, useState } from "react";
import type { CircuitDraftReference } from "@/circuit/domain/circuitReferences.js";
import {
  clearCircuitDraftReferences,
  getCircuitDraftReferenceEventName,
  getCircuitDraftReferences,
  isCircuitDraftReferenceEvent,
  removeCircuitDraftReference,
  setCircuitDraftReferences,
} from "@/lib/circuit/circuitReferenceDrafts.js";

/**
 * 电路引用 chip 的草稿列表（规格 §8）。面板插入引用后经 window 事件通知已挂载的 composer；
 * 列表本体按 workspace 存在模块级 Map 里（与 conversationSelectionReference 同构），
 * 不使用 Zustand：它只是 renderer 输入草稿，不是服务端事实。
 */
export function useCircuitDraftReferences(workspaceKey: string) {
  const [references, setReferences] = useState<readonly CircuitDraftReference[]>(() =>
    getCircuitDraftReferences(workspaceKey),
  );

  useEffect(() => {
    setReferences(getCircuitDraftReferences(workspaceKey));
  }, [workspaceKey]);

  useEffect(() => {
    const handleAdd = (event: Event) => {
      if (!isCircuitDraftReferenceEvent(event)) return;
      if (event.detail.workspaceKey !== workspaceKey) return;
      setReferences(getCircuitDraftReferences(workspaceKey));
    };
    window.addEventListener(getCircuitDraftReferenceEventName(), handleAdd);
    return () => window.removeEventListener(getCircuitDraftReferenceEventName(), handleAdd);
  }, [workspaceKey]);

  const removeReference = useCallback(
    (reference: CircuitDraftReference) =>
      setReferences(removeCircuitDraftReference(workspaceKey, reference)),
    [workspaceKey],
  );

  const clearReferences = useCallback(() => {
    clearCircuitDraftReferences(workspaceKey);
    setReferences([]);
  }, [workspaceKey]);

  /** 发送成功后只移除本次冻结的引用；等待期间新加入的引用属于下一条消息。 */
  const removeReferences = useCallback(
    (submitted: readonly CircuitDraftReference[]) => {
      setReferences((current) => {
        const next = current.filter(
          (item) =>
            !submitted.some(
              (reference) =>
                reference.irId === item.irId &&
                reference.kind === item.kind &&
                reference.name === item.name,
            ),
        );
        setCircuitDraftReferences(workspaceKey, next);
        return next;
      });
    },
    [workspaceKey],
  );

  return { references, removeReference, removeReferences, clearReferences };
}
