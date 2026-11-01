import type { IDisposable } from "@escode/rpc";
import { resolveWorkspaceKey } from "@escode/shared";

export interface HostRemoteWorkspaceContext {
  workspacePath: string;
  workspaceIdentity?: string;
}

/**
 * 保存 shared Host 代理层持有的 workspace 级资源。
 *
 * 当前只剩 `workspace_session_message_send_requested` 的动态事件订阅：Host 需要按 workspace
 * 去重订阅，并在 runtime 释放时解除，否则 listener 会随 Host 寿命单调增长。
 */
export function createHostRemoteWorkspaceProxyState(): {
  ensureWorkspaceSubscription: (
    context: HostRemoteWorkspaceContext,
    subscribe: () => IDisposable,
  ) => boolean;
  clearWorkspace: (context: HostRemoteWorkspaceContext) => void;
} {
  const workspaceSubscriptions = new Map<string, IDisposable>();

  return {
    ensureWorkspaceSubscription(context, subscribe) {
      const workspaceKey = resolveWorkspaceKey(context);
      if (workspaceSubscriptions.has(workspaceKey)) {
        return false;
      }
      workspaceSubscriptions.set(workspaceKey, subscribe());
      return true;
    },

    clearWorkspace(context) {
      const workspaceKey = resolveWorkspaceKey(context);
      workspaceSubscriptions.get(workspaceKey)?.dispose();
      workspaceSubscriptions.delete(workspaceKey);
    },
  };
}
