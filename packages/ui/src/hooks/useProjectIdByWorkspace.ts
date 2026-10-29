import { useEffect, useMemo, useState } from "react";
import type { IServiceAccessor, VirtualProject } from "@escode/services";

/**
 * 项目 root → projectId 的匹配键。项目 roots 与 workspace 路径可能带尾斜杠/分隔符差异，
 * 统一归一化后再比对；不做大小写折叠（POSIX 路径区分大小写）。
 */
export function normalizeWorkspacePathForProjectMatch(path: string): string {
  const normalized = path.replace(/\\/g, "/").replace(/\/+$/, "");
  return normalized.length > 0 ? normalized : "/";
}

/** 只映射本地 Host 的可用项目；同一 root 同时属于多个项目时主 root 优先。 */
export function projectIdByWorkspaceRoots(
  projects: readonly VirtualProject[],
): Record<string, string> {
  const byRoot: Record<string, string> = {};
  const local = projects.filter((project) => !project.deleted && project.hostId === "local");
  // 两遍：先让所有项目的主 root 占位，再补附加 root，避免项目顺序决定归属。
  for (const isPrimary of [true, false]) {
    for (const project of local) {
      for (const root of project.roots) {
        if (root.isPrimary !== isPrimary) continue;
        const key = normalizeWorkspacePathForProjectMatch(root.rootPath);
        if (key in byRoot) continue;
        byRoot[key] = project.id;
      }
    }
  }
  return byRoot;
}

export function projectIdForWorkspacePath(
  byRoot: Record<string, string>,
  workspacePath: string,
): string | null {
  return byRoot[normalizeWorkspacePathForProjectMatch(workspacePath)] ?? null;
}

/**
 * 当前 Host 的项目 root 索引。项目列表由 Host 权威提供；旧 Host 没有 projectService 时返回空表，
 * 草稿按“未归类”处理，不伪造项目归属。
 */
export function useProjectIdByWorkspace(
  services: IServiceAccessor | null | undefined,
): Record<string, string> {
  const [projects, setProjects] = useState<readonly VirtualProject[]>([]);
  const projectService = services?.projectService;
  useEffect(() => {
    let disposed = false;
    if (!projectService) {
      setProjects([]);
      return;
    }
    void projectService
      .readProjects()
      .then((next) => {
        if (!disposed) setProjects(next);
      })
      .catch(() => {
        if (!disposed) setProjects([]);
      });
    return () => {
      disposed = true;
    };
  }, [projectService]);
  return useMemo(() => projectIdByWorkspaceRoots(projects), [projects]);
}
