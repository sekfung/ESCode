import { readFile, writeFile } from "node:fs/promises";
import { NodePersonalProviderConfigRepository } from "@escode/provider-node";
import {
  clearAppData,
  getE2EAppDataPaths,
  waitForWorkspaceApp,
} from "../../../helpers/desktop-app.js";
import { prepareV4ConversationE2E } from "../../../helpers/v4-conversation.js";
import {
  clickLastEnabledSSHDialogButton,
  fillSSHPasswordSettings,
  openSSHRemoteDialog,
  readHostProcesses,
  readSSHRuntimeConfig,
  selectSSHRemoteDirectory,
  clickSSHDialogRetry,
  waitForRemoteProjectTabByPath,
  waitForSSHDirectoryStep,
  waitForSSHError,
  SSH_WORKSPACE_READY_TIMEOUT_MS,
} from "../../../helpers/ssh-remote-p0.js";
import { readRemoteTabs } from "../../../helpers/source-server-rust-remote-state.js";

const CASE_MARKER = "SSH_PROVISIONING_FAILURE";

describe(CASE_MARKER, () => {
  after(async () => {
    await browser.electron.restoreAllMocks();
    await clearAppData();
  });

  it("SSH-P0-CONN-05 首次失败保留安全诊断，恢复凭据后失效默认不阻断连接", async function () {
    this.timeout(20 * 60_000);
    const config = readSSHRuntimeConfig(CASE_MARKER);
    const workspacePath = config.workspacePaths[0];
    await prepareV4ConversationE2E({ skipProvider: true });
    const hosts = await readHostProcesses();
    const paths = getE2EAppDataPaths();
    const credentials = await readFile(paths.credentialsFile, "utf8");
    const invalidCredential = "enc:v1:E2E_INVALID_PROVISIONING_CREDENTIAL";
    try {
      await writeFile(
        paths.credentialsFile,
        JSON.stringify({
          ...JSON.parse(credentials),
          "oauth:bigmodel:refresh_token": invalidCredential,
        }),
      );
      await openSSHRemoteDialog(CASE_MARKER);
      await fillSSHPasswordSettings(config);
      await clickLastEnabledSSHDialogButton(CASE_MARKER);
      // Rust Host 首连屏障现在返回结构化 provider-provisioning-failed；原始远端原因
      // 保留在折叠的 Details 中，可见错误只显示本地化标题。可见文案不得泄露坏凭据值。
      const visibleError = await waitForSSHError(CASE_MARKER);
      expect(visibleError).not.toContain(invalidCredential);
      const text = await browser.execute(
        () => document.querySelector('[data-testid="ssh-dialog"]')?.textContent ?? "",
      );
      expect(text).toContain("source-read-failed");
      expect(text).not.toContain(invalidCredential);
      // S5 起 SSH 远端 workspace identity 由 Rust Host 绑定为 remote:server:*；
      // 失败场景只能按真实路径断言没有生成远端 tab，不能再用 transport target 推导 identity。
      expect(
        (await readRemoteTabs()).filter(
          (tab) => tab.workspacePath === workspacePath && tab.remoteSessionId,
        ),
      ).toEqual([]);
    } finally {
      await writeFile(paths.credentialsFile, credentials);
    }

    const defaultModelSelection = {
      providerId: "e2e-removed-provider",
      modelId: "e2e-removed-model",
    };
    const personal = new NodePersonalProviderConfigRepository({
      filePath: paths.configFile,
      pollingIntervalMs: false,
    });
    try {
      await personal.update((current) => ({ ...current, defaultModelSelection }));
      await clickSSHDialogRetry(CASE_MARKER);
      await waitForSSHDirectoryStep(CASE_MARKER);
      await selectSSHRemoteDirectory(workspacePath, CASE_MARKER);
      await waitForWorkspaceApp(workspacePath, SSH_WORKSPACE_READY_TIMEOUT_MS);
      await waitForRemoteProjectTabByPath({
        caseMarker: CASE_MARKER,
        expectedPath: workspacePath,
      });
      expect((await personal.read()).defaultModelSelection).toEqual(defaultModelSelection);
      expect(await readHostProcesses()).toEqual(hosts);
    } finally {
      personal.dispose();
    }
  });
});
