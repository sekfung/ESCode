import { clearAppData } from "../helpers/desktop-app.js";
import { readRemoteTabs } from "../helpers/source-server-rust-remote-state.js";
import { prepareV4ConversationE2E } from "../helpers/v4-conversation.js";
import {
  clickLastEnabledSSHDialogButton,
  completeSSHDeviceConnection,
  fillSSHPasswordSettings,
  openSSHRemoteDialog,
  readHostProcesses,
  readSSHRuntimeConfig,
  waitForSSHError,
} from "../helpers/ssh-remote-p0.js";

const CASE_MARKER = "SSH_P0_CONNECTION";

describe(`${CASE_MARKER}: SSH password 连接 P0`, () => {
  after(async () => {
    await browser.electron.restoreAllMocks();
    await clearAppData();
  });

  it("SSH-P0-CONN-02/01 错误密码失败后，以正确密码重试并选择远端目录", async function () {
    this.timeout(20 * 60_000);
    const config = readSSHRuntimeConfig(CASE_MARKER);
    const workspacePath = config.workspacePaths[0];
    await prepareV4ConversationE2E({ skipProvider: true });
    // S6 起本地 window Host 与远程 SSH 都由 App 级 Rust Host 服务。
    const initialHosts = await readHostProcesses();
    expect(initialHosts.length).toBeGreaterThanOrEqual(1);

    await openSSHRemoteDialog(CASE_MARKER);
    const invalidPassword = `${config.password}-e2e-invalid`;
    await fillSSHPasswordSettings({
      ...config,
      password: invalidPassword,
    });
    await clickLastEnabledSSHDialogButton(CASE_MARKER);
    const errorText = await waitForSSHError(CASE_MARKER);
    expect(errorText).not.toContain(config.password);
    expect(errorText).not.toContain(invalidPassword);
    expect((await readRemoteTabs()).some((tab) => tab.workspacePath === workspacePath)).toBe(false);
    // 直接 SSH 首连由 App 级 Rust Host 解析 Profile；认证失败不会懒创建
    // per-window Node Host，也不会留下可用的 remote session。
    const hostsAfterFailure = await readHostProcesses();
    expect(hostsAfterFailure).toEqual(expect.arrayContaining(initialHosts));
    expect(hostsAfterFailure.length).toBe(initialHosts.length);

    await closeSSHDialogAfterFailure();
    await openSSHRemoteDialog(CASE_MARKER);
    await fillSSHPasswordSettings(config);
    // S5-e 后「添加设备」连接成功即关闭向导，改由项目选择器承接目录。
    const tab = await completeSSHDeviceConnection({
      caseMarker: CASE_MARKER,
      workspacePath,
    });
    expect(tab.remoteKind).toBe("ssh");
    expect(tab.remoteHost).toBe(config.host);
    expect(tab.secretPersistedInTab).toBe(false);
    const hostsAfterConnect = await readHostProcesses();
    expect(hostsAfterConnect.length).toBe(initialHosts.length);
    expect(hostsAfterConnect).toEqual(expect.arrayContaining(initialHosts));
  });
});

async function closeSSHDialogAfterFailure(): Promise<void> {
  await browser.keys("Escape");
  await browser.waitUntil(
    () => browser.execute(() => !document.querySelector('[data-testid="ssh-dialog"]')),
    {
      timeout: 15_000,
      timeoutMsg: `${CASE_MARKER}: 失败后的 SSH 向导没有关闭`,
    },
  );
}
