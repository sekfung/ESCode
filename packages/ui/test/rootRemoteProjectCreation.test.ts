// 远端项目入口：Host 先解析目录/身份，再由 Root attachment 并派发跨 Root 打开意图。
// 这里用源码契约锁定两步接线，避免再次退化为「静默落到活动 workspace」。
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

function source(relative: string): string {
  return readFileSync(new URL(relative, import.meta.url), "utf8").replaceAll("\r\n", "\n");
}

describe("Root 远端项目创建", () => {
  it("handleCreateTask 先调用 prepareSession 再交给跨 Root opener", () => {
    const hook = source("../src/root/useRootWorkspaceActions.ts");

    expect(hook).toContain("services.projectService.prepareSession({");
    expect(hook).toContain("projectId,");
    expect(hook).toContain("hostId: requestedHostId,");
    expect(hook).toContain("await openRemoteProjectWorkspace({");
    expect(hook).toContain("workspaceIdentity: prepared.workspaceIdentity,");
    expect(hook).toContain("expectedMembershipId: request.remoteProjectMembershipId");
    // 远端请求不能再直接失败，也不能落到本地活动 workspace。
    expect(hook).not.toContain("project.remote-navigation-unavailable");
    expect(hook).not.toContain("本入口尚未接线");
  });

  it("Root 负责 attachment 并把 open-project 意图交给 RemoteWorkspaceController", () => {
    const root = source("../src/Root.tsx");

    expect(root).toContain("openRemoteProjectWorkspace: openPreparedRemoteProjectWorkspace");
    expect(root).toContain("await attachRemoteProjectWorkspace({");
    expect(root).toContain("remoteService: services.remoteProjectService");
    expect(root).toContain("remoteWorkspaceController.dispatch({");
    expect(root).toContain('type: "open-project"');
    expect(root).toContain("draftProjectId: target.projectId");
    expect(root).toContain("preparedWorkspaceIdentity: target.workspaceIdentity");
  });

  it("创建完成后按发起时租约回调 onComplete，失败回传 onError", () => {
    const hook = source("../src/root/useRootWorkspaceActions.ts");

    expect(hook).toMatch(
      /if \(!isCurrent\(\)\) return;\s*setWorkspaceActionError\(null\);\s*if \(typeof request === "object"\) request\.onComplete\?\.\(\);/,
    );
    expect(hook).toMatch(
      /logger\.error\("\[Root\] 准备远端项目任务失败", error\);\s*if \(typeof request === "object"\) request\.onError\?\.\(error\);/,
    );
  });
});
