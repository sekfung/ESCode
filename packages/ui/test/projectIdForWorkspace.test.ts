import { describe, expect, it } from "vitest";
import type { VirtualProject } from "@escode/services";
import {
  projectIdByWorkspaceRoots,
  projectIdForWorkspacePath,
} from "@/hooks/useProjectIdByWorkspace.js";

function project(
  id: string,
  roots: Array<{ rootPath: string; isPrimary?: boolean }>,
  overrides: Partial<VirtualProject> = {},
): VirtualProject {
  return {
    id,
    hostId: "local",
    name: id,
    nameOverridden: false,
    roots: roots.map((root, position) => ({
      rootPath: root.rootPath,
      isPrimary: root.isPrimary ?? false,
      position,
    })),
    position: 0,
    pinned: false,
    deleted: false,
    createdAt: 0,
    updatedAt: 0,
    ...overrides,
  };
}

describe("项目归属派生", () => {
  it("按 root 映射 projectId，主 root 优先于其他项目", () => {
    const byRoot = projectIdByWorkspaceRoots([
      project("secondary", [{ rootPath: "/workspace" }]),
      project("primary", [
        { rootPath: "/workspace", isPrimary: true },
        { rootPath: "/workspace-extra" },
      ]),
    ]);
    expect(byRoot["/workspace"]).toBe("primary");
    expect(byRoot["/workspace-extra"]).toBe("primary");
  });

  it("忽略已删除与远端项目", () => {
    const byRoot = projectIdByWorkspaceRoots([
      project("deleted", [{ rootPath: "/deleted", isPrimary: true }], { deleted: true }),
      project("remote", [{ rootPath: "/remote", isPrimary: true }], { hostId: "remote:server:s" }),
      project("local", [{ rootPath: "/local", isPrimary: true }]),
    ]);
    expect(byRoot["/deleted"]).toBeUndefined();
    expect(byRoot["/remote"]).toBeUndefined();
    expect(byRoot["/local"]).toBe("local");
  });

  it("路径尾斜杠与反斜杠不影响匹配", () => {
    const byRoot = projectIdByWorkspaceRoots([
      project("p", [{ rootPath: "/workspace/project/", isPrimary: true }]),
    ]);
    expect(projectIdForWorkspacePath(byRoot, "/workspace/project")).toBe("p");
    expect(projectIdForWorkspacePath(byRoot, "/workspace/other")).toBeNull();
  });
});
