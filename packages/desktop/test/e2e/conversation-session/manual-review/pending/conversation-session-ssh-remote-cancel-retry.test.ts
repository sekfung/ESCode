import { TID_CONFIRM_DIALOG_CONFIRM } from "@escode/shared";
import {
  clearAppData,
  clickTestIdByDom,
  waitForWorkspaceApp,
} from "../../../helpers/desktop-app.js";
import { prepareV4ConversationE2E } from "../../../helpers/v4-conversation.js";
import {
  clickLastEnabledSSHDialogButton,
  createFirstConnectionStallingSSHProxy,
  fillSSHPasswordSettings,
  openSSHRemoteDialog,
  readHostProcesses,
  readSSHRuntimeConfig,
  selectSSHRemoteDirectory,
  waitForRemoteProjectTabByPath,
  waitForSSHDialogClosed,
  waitForSSHDirectoryStep,
  type FirstConnectionStallingSSHProxy,
  SSH_WORKSPACE_READY_TIMEOUT_MS,
} from "../../../helpers/ssh-remote-p0.js";
import { readRemoteTabs } from "../../../helpers/source-server-rust-remote-state.js";

const CASE_MARKER = "E2E_SSH_P0_CANCEL_RETRY";
const CONNECTION_CLOSE_TIMEOUT_MS = 30_000;

describe(`${CASE_MARKER}: SSH pending cancel 后立即重试`, () => {
  let proxy: FirstConnectionStallingSSHProxy | null = null;

  after(async () => {
    try {
      await browser.electron.restoreAllMocks();
      await clearAppData();
    } finally {
      await proxy?.close();
      proxy = null;
    }
  });

  it("SSH-P0-CONN-03 停止 pending 连接后，同 target 使用新凭据重试", async function () {
    this.timeout(20 * 60_000);
    const config = readSSHRuntimeConfig(CASE_MARKER);
    const workspacePath = config.workspacePaths[0];
    proxy = await createFirstConnectionStallingSSHProxy(config, CASE_MARKER);
    const proxiedConfig = {
      ...config,
      host: "127.0.0.1",
      port: proxy.port,
    };
    await prepareV4ConversationE2E({ skipProvider: true });
    const initialHosts = await readHostProcesses();
    expect(initialHosts).toHaveLength(1);
    const initialTabs = await readRemoteTabs();

    await openSSHRemoteDialog(CASE_MARKER);
    await fillSSHPasswordSettings({
      ...proxiedConfig,
      password: "cancelled-attempt",
    });
    await clickLastEnabledSSHDialogButton(CASE_MARKER);
    await proxy.firstConnectionAccepted;

    await browser.keys("Escape");
    await clickTestIdByDom(TID_CONFIRM_DIALOG_CONFIRM, {
      timeout: 15_000,
      timeoutMsg: `${CASE_MARKER}: pending SSH 连接没有显示停止确认`,
    });
    await waitForPromise(
      proxy.waitForFirstConnectionClosed(),
      CONNECTION_CLOSE_TIMEOUT_MS,
      `${CASE_MARKER}: 取消后首个 SSH socket 没有关闭`,
    );
    // 取消契约（spec §12.2）：Escape + 确认走「停止连接」，向导直接关闭且不展示错误页，
    // 也不产生 logical session；改凭据重试必须重新进入向导，而不是复用被取消的 pending 目标。
    await waitForSSHDialogClosed(CASE_MARKER);
    expect(await readRemoteTabs()).toEqual(initialTabs);

    await openSSHRemoteDialog(CASE_MARKER);
    await fillSSHPasswordSettings(proxiedConfig);
    await clickLastEnabledSSHDialogButton(CASE_MARKER);
    await waitForSSHDirectoryStep(CASE_MARKER);
    await selectSSHRemoteDirectory(workspacePath, CASE_MARKER);
    await waitForWorkspaceApp(workspacePath, SSH_WORKSPACE_READY_TIMEOUT_MS);

    const tab = await waitForRemoteProjectTabByPath({
      caseMarker: CASE_MARKER,
      expectedPath: workspacePath,
    });
    expect(tab.remoteSessionId).toBeTruthy();
    expect(tab.secretPersistedInTab).toBe(false);
    expect(await readHostProcesses()).toEqual(initialHosts);
  });
});

async function waitForPromise<T>(
  promise: Promise<T>,
  timeoutMs: number,
  timeoutMessage: string,
): Promise<T> {
  let timeout: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      promise,
      new Promise<never>((_resolve, reject) => {
        timeout = setTimeout(() => reject(new Error(timeoutMessage)), timeoutMs);
      }),
    ]);
  } finally {
    if (timeout) clearTimeout(timeout);
  }
}
