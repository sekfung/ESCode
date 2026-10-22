import { createContext, useContext, type ReactNode } from "react";
import type { WorkspaceVcsController } from "@/svn/useSvnWorkspace.js";

const SvnWorkspaceContext = createContext<WorkspaceVcsController | null>(null);

/**
 * 把当前可见 workspace 的 VCS 判定与 SVN 状态下发给面板、状态区、分支入口与文件树。
 * 所有者是 App 中的 useWorkspaceVcs，这里只是传递通道，不持有第二份状态。
 */
export function SvnWorkspaceProvider({
  value,
  children,
}: {
  value: WorkspaceVcsController;
  children: ReactNode;
}) {
  return <SvnWorkspaceContext.Provider value={value}>{children}</SvnWorkspaceContext.Provider>;
}

export function useWorkspaceVcsContext(): WorkspaceVcsController | null {
  return useContext(SvnWorkspaceContext);
}

/** 当前 workspace 是 SVN 工作副本时返回控制器，否则为 null（Git / 无版本控制 / 判定中）。 */
export function useSvnWorkspaceContext() {
  return useContext(SvnWorkspaceContext)?.svn ?? null;
}

/** 判定完成且确定是 SVN 时为 true；判定中返回 null，调用方据此避免先渲染 Git 再切换。 */
export function useIsSvnWorkspace(): boolean | null {
  const controller = useContext(SvnWorkspaceContext);
  if (!controller) {
    return false;
  }
  if (!controller.vcs) {
    return null;
  }
  return controller.vcs.kind === "svn";
}
