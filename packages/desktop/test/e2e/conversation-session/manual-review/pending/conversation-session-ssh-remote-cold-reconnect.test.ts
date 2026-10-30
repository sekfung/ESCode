import { clearAppData } from "../../../helpers/desktop-app.js";
import { selectUpstreamProviderModelById } from "../../../helpers/upstream-provider.js";
import { selectV4TaskById } from "../../../helpers/v4-conversation.js";
import {
  connectSSHWorkspaceAndSendFirstTurn,
  prepareSSHLifecycle,
  readTaskRowInWorkspace,
  sendSSHFollowTurn,
  waitForRendererAfterColdRestart,
  type SSHLifecycleProvider,
  type SSHLifecycleTask,
  SSH_LIFECYCLE_MODEL_ID,
} from "../../../helpers/ssh-remote-lifecycle.js";
import { readHostProcesses, waitForRemoteTab } from "../../../helpers/ssh-remote-p0.js";

const CASE_MARKER = "E2E_SSH_P0_COLD_RECONNECT";

describe(`${CASE_MARKER}: SSH cold restart auto reconnect`, () => {
  let provider: SSHLifecycleProvider | null = null;

  after(async () => {
    await provider?.tunnel.close().catch(() => undefined);
    provider = null;
    await browser.electron.restoreAllMocks();
    await clearAppData();
  });

  it("SSH-P0-LIFE-01 App 冷重启后保留 disconnected workspace 并手动恢复原 task", async function () {
    this.timeout(35 * 60_000);
    const prepared = await prepareSSHLifecycle(CASE_MARKER);
    provider = prepared.provider;
    const [workspaceA, workspaceB] = prepared.config.workspacePaths;
    const taskA = await connectSSHWorkspaceAndSendFirstTurn({
      caseMarker: CASE_MARKER,
      config: prepared.config,
      label: "A",
      provider,
      workspacePath: workspaceA,
    });
    const taskB = await connectSSHWorkspaceAndSendFirstTurn({
      caseMarker: CASE_MARKER,
      config: prepared.config,
      label: "B",
      provider,
      workspacePath: workspaceB,
    });
    const oldHosts = await readHostProcesses();

    // browser.reloadSession 才会重启 Electron/Main/Host/CLI；renderer reload 不足以证明冷恢复。
    await browser.reloadSession();
    await waitForRendererAfterColdRestart(CASE_MARKER);
    // 默认开启冷启动自动重连（remoteStartupAutoReconnectEnabled ?? true）：
    // 不点手动重连，等待 A/B 各自自动拿到新的 remoteSessionId。
    const reconnectedA = await waitForRemoteTab({
      caseMarker: CASE_MARKER,
      expectedIdentity: taskA.identity,
      expectedPath: workspaceA,
      requireActive: false,
      timeoutMs: 90_000,
    });
    const reconnectedB = await waitForRemoteTab({
      caseMarker: CASE_MARKER,
      expectedIdentity: taskB.identity,
      expectedPath: workspaceB,
      requireActive: false,
      timeoutMs: 90_000,
    });
    expect(reconnectedA.remoteSessionId).not.toBe(taskA.remoteSessionId);
    expect(reconnectedB.remoteSessionId).not.toBe(taskB.remoteSessionId);
    expect(reconnectedA.secretPersistedInTab).toBe(false);
    expect(reconnectedB.secretPersistedInTab).toBe(false);
    // 冷启动后本地 window Host 重建，远端 scope 仍只允许一个 Node Host。
    expect(
      (await readHostProcesses()).filter((host) =>
        host.name.startsWith("escode-host-remote-"),
      ).length,
    ).toBe(1);

    // 模型选择作用于当前激活 workspace：先激活 A 的原任务。
    await selectV4TaskById(taskA.taskId, 30_000);
    await selectUpstreamProviderModelById(SSH_LIFECYCLE_MODEL_ID, {
      includePlainModelFallback: false,
      providerId: provider.id,
      providerName: provider.name,
    });
    const resumedA: SSHLifecycleTask = {
      ...taskA,
      remoteSessionId: reconnectedA.remoteSessionId!,
    };
    await readTaskRowInWorkspace(resumedA, CASE_MARKER);
    await sendSSHFollowTurn(resumedA, CASE_MARKER, taskB.firstPrompt);
    expect(await readHostProcesses()).not.toEqual(oldHosts);
  });
});
