/**
 * VCS 覆盖值变更通知。覆盖值的所有者是 Host 的 workspace-vcs 服务；设置页写入后只广播
 * “某个 workspace 身份 key 已变”，由 App 中的 useWorkspaceVcs 重新读取，不传递状态本身。
 */
const WORKSPACE_VCS_CHANGED_EVENT = "escode:workspace-vcs-changed";

export function notifyWorkspaceVcsChanged(workspaceKey: string): void {
  if (typeof window === "undefined") {
    return;
  }
  window.dispatchEvent(
    new CustomEvent<string>(WORKSPACE_VCS_CHANGED_EVENT, { detail: workspaceKey }),
  );
}

export function subscribeWorkspaceVcsChanged(listener: (workspaceKey: string) => void): () => void {
  if (typeof window === "undefined") {
    return () => undefined;
  }
  const handler = (event: Event) => {
    const detail = (event as CustomEvent<unknown>).detail;
    if (typeof detail === "string") {
      listener(detail);
    }
  };
  window.addEventListener(WORKSPACE_VCS_CHANGED_EVENT, handler);
  return () => window.removeEventListener(WORKSPACE_VCS_CHANGED_EVENT, handler);
}
