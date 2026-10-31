import { posix } from "node:path";
import { TID_COMPOSER_WORKSPACE_TRIGGER } from "@escode/shared";
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
    // 标记子目录用于证明目录选择器确实 readdir 了目标路径，而不是停在初始 home。
    const markerEntry = `e2e-marker-${suffix}`;
    const created = await runSSHCommand(config, `mkdir -p '${remoteFixturePath}/${markerEntry}'`);
    expect(created.code).toBe(0);
    expect(created.stderr).toBe("");

    await skipOccupationOnboardingIfPresent();
    await prepareV4ConversationE2E({ skipProvider: true });

    // 走 composer「添加设备」：连接只证明设备可用，随后由项目选择器选目录并新建项目。
    await openSSHDevicePicker(CASE_MARKER);
    await fillSSHPasswordSettings(config);
    await clickLastEnabledSSHDialogButton(CASE_MARKER);
    await waitForSSHDialogClosed(CASE_MARKER);
    await selectRemoteFolderProjectDirectory(remoteFixturePath, CASE_MARKER, markerEntry);

    const projectName = `E2E remote project ${suffix}`;
    const nameInput = await $('[data-testid="virtual-project-name"]');
    try {
      await nameInput.waitForDisplayed({
        timeout: 30_000,
        timeoutMsg: `${CASE_MARKER}: 新远端目录没有打开项目创建表单`,
      });
    } catch (error) {
      const snapshot = await browser
        .execute(() => ({
          dialogText: (
            document.querySelector('[data-testid="chat-remote-folder"]')?.textContent ?? ""
          ).slice(0, 1_200),
          visibleTestIds: Array.from(document.querySelectorAll("[data-testid]"))
            .filter((element) => (element as HTMLElement).getClientRects().length > 0)
            .map((element) => element.getAttribute("data-testid")),
          bodyText: (document.body.innerText ?? "").slice(0, 1_200),
        }))
        .catch(() => null);
      throw new Error(
        `${error instanceof Error ? error.message : String(error)} [dialog: ${
          snapshot?.dialogText.replaceAll("\n", " | ") ?? "unavailable"
        }] [visible testids: ${snapshot?.visibleTestIds.join(", ") ?? "unavailable"}] [body: ${
          snapshot?.bodyText.replaceAll("\n", " | ") ?? "unavailable"
        }]`,
      );
    }
    await nameInput.setValue(projectName);
    const saveButton = await $('[data-testid="virtual-project-save"]');
    await saveButton.waitForEnabled({
      timeout: 10_000,
      timeoutMsg: `${CASE_MARKER}: 项目名已填写但保存按钮不可用`,
    });
    await saveButton.click();

    // 新项目首次保存会先要求确认目录信任；不确认则保存流程一直挂起，工作区不会出现。
    const trustConfirm = await $('[data-testid="project-folder-trust-confirm"]');
    await trustConfirm.waitForDisplayed({
      timeout: 30_000,
      timeoutMsg: `${CASE_MARKER}: 保存后没有出现目录信任确认`,
    });
    const trustPath = (await $('[data-testid="project-folder-trust-path"]').getText()).trim();
    expect(trustPath).toBe(remoteFixturePath);
    await trustConfirm.click();

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

    // 当前桌面 shell 挂的是 WorkspaceSidebar，项目目录行由 composer 的 ProjectComposerMenu 渲染；
    // ProjectSidebar（ProjectNode）在本 fork 未挂载，不能再用 [data-project-id] 直接查整页。
    const workspaceTrigger = await $(`[data-testid="${TID_COMPOSER_WORKSPACE_TRIGGER}"]`);
    await workspaceTrigger.waitForDisplayed({
      timeout: 30_000,
      timeoutMsg: `${CASE_MARKER}: 草稿 composer 没有显示项目入口`,
    });
    expect(await workspaceTrigger.getText()).toContain(projectName);
    await workspaceTrigger.click();

    await browser.waitUntil(
      () =>
        browser.execute(
          (projectId, expectedName) => {
            const menu = document.querySelector<HTMLElement>(
              '[data-testid="composer-project-menu"]',
            );
            if (!menu?.getClientRects().length) return false;
            const row = menu.querySelector<HTMLElement>(`[data-project-id="${projectId}"]`);
            return Boolean(
              row?.getClientRects().length && (row.textContent ?? "").includes(expectedName),
            );
          },
          draft.draftProjectId,
          projectName,
        ),
      {
        timeout: 30_000,
        interval: 250,
        timeoutMsg: `${CASE_MARKER}: 项目目录里没有新保存的远端项目`,
      },
    );
  });
});
