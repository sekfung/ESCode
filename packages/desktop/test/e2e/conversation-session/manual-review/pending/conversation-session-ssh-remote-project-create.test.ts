import { posix } from "node:path";
import { clearAppData, waitForWorkspaceApp } from "../../../helpers/desktop-app.js";
import { skipOccupationOnboardingIfPresent } from "../../../helpers/occupation-onboarding.js";
import { prepareV4ConversationE2E } from "../../../helpers/v4-conversation.js";
import {
  clickLastEnabledSSHDialogButton,
  fillSSHPasswordSettings,
  openSSHDevicePicker,
  readSingleSSHWorkspaceRuntimeConfig,
  runSSHCommand,
  selectRemoteFolderProjectDirectory,
  waitForSSHDialogClosed,
  waitForRemoteProjectTabByPath,
  SSH_WORKSPACE_READY_TIMEOUT_MS,
} from "../../../helpers/ssh-remote-p0.js";

const CASE_MARKER = "E2E_SSH_P0_PROJECT_CREATE";

describe(`${CASE_MARKER}: 远端目录首次创建项目并落到草稿`, () => {
  let remoteFixturePath: string | null = null;

  after(async () => {
    if (remoteFixturePath) {
      const config = readSingleSSHWorkspaceRuntimeConfig(CASE_MARKER);
      await runSSHCommand(config, `rm -rf -- '${remoteFixturePath}'`).catch(() => undefined);
      remoteFixturePath = null;
    }
    await browser.electron.restoreAllMocks();
    await clearAppData();
  });

  it("SSH-P0-PROJECT-01 连接新远端目录，保存项目后打开对应草稿", async function () {
    this.timeout(30 * 60_000);
    const config = readSingleSSHWorkspaceRuntimeConfig(CASE_MARKER);
    const suffix = `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
    remoteFixturePath = posix.join(config.workspacePath, ".escode-e2e", `ssh-p0-project-${suffix}`);
    if (!remoteFixturePath.startsWith(`${config.workspacePath}/`)) {
      throw new Error(`${CASE_MARKER}: 非法远端 fixture path`);
    }
    const created = await runSSHCommand(config, `mkdir -p '${remoteFixturePath}'`);
    expect(created.code).toBe(0);
    expect(created.stderr).toBe("");

    await skipOccupationOnboardingIfPresent();
    await prepareV4ConversationE2E({ skipProvider: true });

    // 走 composer「添加设备」：连接只证明设备可用，随后由项目选择器选目录并新建项目。
    await openSSHDevicePicker(CASE_MARKER);
    await fillSSHPasswordSettings(config);
    await clickLastEnabledSSHDialogButton(CASE_MARKER);
    await waitForSSHDialogClosed(CASE_MARKER);
    await selectRemoteFolderProjectDirectory(remoteFixturePath, CASE_MARKER);

    const projectName = `E2E remote project ${suffix}`;
    const nameInput = await $('[data-testid="virtual-project-name"]');
    await nameInput.waitForDisplayed({
      timeout: 30_000,
      timeoutMsg: `${CASE_MARKER}: 新远端目录没有打开项目创建表单`,
    });
    await nameInput.setValue(projectName);
    const saveButton = await $('[data-testid="virtual-project-save"]');
    await saveButton.waitForEnabled({
      timeout: 10_000,
      timeoutMsg: `${CASE_MARKER}: 项目名已填写但保存按钮不可用`,
    });
    await saveButton.click();

    await browser.waitUntil(
      () => browser.execute(() => !document.querySelector('[data-testid="chat-remote-folder"]')),
      {
        timeout: SSH_WORKSPACE_READY_TIMEOUT_MS,
        interval: 250,
        timeoutMsg: `${CASE_MARKER}: 创建项目后远端目录对话框没有关闭`,
      },
    );
    await waitForWorkspaceApp(remoteFixturePath, SSH_WORKSPACE_READY_TIMEOUT_MS);

    const tab = await waitForRemoteProjectTabByPath({
      caseMarker: CASE_MARKER,
      expectedPath: remoteFixturePath,
    });
    expect(tab.workspaceIdentity).toMatch(/^remote:server:/u);
    expect(tab.workspaceIdentity).not.toContain(config.password);
    expect(tab.remoteKind).toBe("ssh");
    expect(tab.remoteSessionId).toBeTruthy();
    expect(tab.secretPersistedInTab).toBe(false);
    expect(tab.activeWorkspacePath).toBe(remoteFixturePath);
    expect(tab.activeWorkspaceIdentity).toBe(tab.workspaceIdentity);

    const draft = await browser.execute(
      ({ workspacePath, workspaceIdentity }) => {
        type WorkspaceState = {
          activeTaskId?: string | null;
          draftProjectId?: string | null;
          draftTaskGroupId?: string | null;
        };
        const store = (
          window as Window & {
            __escodeSessionStoreE2E?: {
              getState?: () => {
                getWorkspaceState?: (path: string, identity?: string) => WorkspaceState;
              };
            };
          }
        ).__escodeSessionStoreE2E;
        const workspace = store
          ?.getState?.()
          .getWorkspaceState?.(workspacePath, workspaceIdentity ?? undefined);
        return {
          activeTaskId: workspace?.activeTaskId ?? null,
          draftProjectId: workspace?.draftProjectId ?? null,
          draftTaskGroupId: workspace?.draftTaskGroupId ?? null,
        };
      },
      { workspacePath: remoteFixturePath, workspaceIdentity: tab.workspaceIdentity },
    );
    expect(draft.activeTaskId).toBeNull();
    expect(draft.draftProjectId).toBeTruthy();
    expect(draft.draftTaskGroupId).toBeNull();

    await browser.waitUntil(
      () =>
        browser.execute(
          (projectId, expectedName) => {
            const row = document.querySelector<HTMLElement>(`[data-project-id="${projectId}"]`);
            return Boolean(row?.getClientRects().length && row.innerText.includes(expectedName));
          },
          draft.draftProjectId,
          projectName,
        ),
      {
        timeout: 30_000,
        interval: 250,
        timeoutMsg: `${CASE_MARKER}: 新项目没有出现在项目侧栏`,
      },
    );
  });
});
