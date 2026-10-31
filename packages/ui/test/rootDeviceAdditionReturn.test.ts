// 项目菜单的「添加设备」只发起连接；Root 负责在对话框期间持有返回目标。
// 用源码契约锁定 completion 回传，避免连接成功后仍进入目录步骤把向导卡住。
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

function source(relative: string): string {
  return readFileSync(new URL(relative, import.meta.url), "utf8").replaceAll("\r\n", "\n");
}

describe("Root 设备添加返回目标", () => {
  it("把 completion 与设备管理回调传给 SSHDialog", () => {
    const root = source("../src/Root.tsx");

    expect(root).toContain("completion={remoteConnectionOpenPreference?.completion}");
    expect(root).toContain("onDeviceConnected={async (sourceServerId, canDeliver) => {");
    expect(root).toContain(
      'await finishDeviceAddition({ kind: "connected", sourceServerId }, false, canDeliver);',
    );
    expect(root).toContain("onCancelled={(background) => {");
  });

  it("只向仍有效的发起 tab/草稿回传结果", () => {
    const root = source("../src/Root.tsx");

    expect(root).toContain(
      "deviceAdditionReturn.current === pending && pending.target.isCurrent()",
    );
    expect(root).toContain("pending.workspaceKey !== workspaceKey");
    expect(root).toContain("const valid = await pending.target.canFinish(result);");
    expect(root).toContain('if (result.kind === "connected" && !canDeliver()) {');
    expect(root).toContain("return false;");
    expect(root).toContain("tabs.activateTab(pending.tabId);");
  });

  it("连接偏好完整透传，避免已选设备/目录意图丢失", () => {
    const root = source("../src/Root.tsx");

    expect(root).toContain(
      "preferredSourceServerId={remoteConnectionOpenPreference?.preferredSourceServerId}",
    );
    expect(root).toContain(
      "preferredProfileId={remoteConnectionOpenPreference?.preferredProfileId}",
    );
    expect(root).toContain(
      "preferredDirectoryPath={remoteConnectionOpenPreference?.preferredDirectoryPath}",
    );
    expect(root).toContain("preferredTarget={remoteConnectionOpenPreference?.preferredTarget}");
  });
});
