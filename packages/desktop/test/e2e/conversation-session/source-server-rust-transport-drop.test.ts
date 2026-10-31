import { randomUUID } from "node:crypto";
import { buildServerRemoteWorkspaceIdentity } from "@escode/shared";
import {
  clearAppData,
  seedSettings,
  waitForDefaultWorkspaceReady,
  waitForWorkspaceApp,
} from "../helpers/desktop-app.js";
import {
  getUpstreamRequestRecordCount,
  waitForUpstreamRequest,
} from "../helpers/conversation-session-network.js";
import { reloadElectronSessionSafely } from "../helpers/e2e-electron-reload.js";
import {
  openRemoteProfile,
  saveCredential,
  selectRemoteDirectoryByPath,
  waitForProfileState,
} from "../helpers/source-server-rust-remote.js";
import {
  SSH_LIFECYCLE_TURN_TIMEOUT_MS,
  sendSSHFollowTurn,
  type SSHLifecycleTask,
} from "../helpers/ssh-remote-lifecycle.js";
import {
  selectUpstreamProviderModelById,
} from "../helpers/upstream-provider.js";
import {
  prepareV4ConversationE2E,
  sendV4PromptAndWaitAccepted,
  startNewV4Draft,
  waitForV4AssistantMessageContaining,
  waitForV4ConversationState,
} from "../helpers/v4-conversation.js";
import {
  readHostProcessIdentities,
  readRemoteTabs,
  waitForRemoteSession,
} from "../helpers/source-server-rust-remote-state.js";
import {
  killRustHostTransport,
  removeRemoteSourceServerProvider,
} from "../helpers/source-server-rust-transport.js";
import { seedCustomOpenAIChatCompletionsProvider } from "../helpers/custom-openai-provider-store.js";
import {
  createSSHReplayTunnel,
  readSSHRuntimeConfig,
  waitForSSHDirectoryStep,
  type ReplayTunnel,
  type SSHConnectionConfig,
} from "../helpers/ssh-remote-p0.js";
import { readOrSeedRemoteSourceServerId } from "../helpers/source-server-remote-identity.js";

const CASE_MARKER = "SOURCE_SERVER_RUST_TRANSPORT_DROP";
const WORKSPACE_READY_TIMEOUT_MS = 2 * 60_000;
const REMOTE_PROVIDER_ID = "e2e-rust-transport-drop-provider";
const REMOTE_PROVIDER_NAME = "Rust Transport Drop";
const REMOTE_MODEL_ID = "e2e-rust-transport-drop-model";

describe(`${CASE_MARKER}: Rust Host 远端链路中断后原位重连`, () => {
  let replayTunnel: ReplayTunnel | null = null;
  let remoteProviderConfig: SSHConnectionConfig | null = null;

  after(async () => {
    if (remoteProviderConfig) {
      await removeRemoteSourceServerProvider(
        remoteProviderConfig,
        REMOTE_PROVIDER_ID,
        REMOTE_MODEL_ID,
      ).catch(() => undefined);
      remoteProviderConfig = null;
    }
    await replayTunnel?.close().catch(() => undefined);
    replayTunnel = null;
    await browser.electron.restoreAllMocks();
    await clearAppData();
  });

  it("真实断开 SSH transport 后进入 retrying 并自动恢复 online，仍由 Rust Host 服务且不启动 Node Host", async function () {
    if (!process.env.ESCODE_E2E_SSH_HOST?.trim()) {
      this.skip();
    }
    // 该用例经 /proc 定位 Rust Host 的 ssh 子进程再 SIGKILL 模拟链路中断；Windows/macOS
    // 没有 /proc，跨平台主链改由 install / restart-recovery 用例覆盖（S4-i）。
    if (process.platform !== "linux") {
      this.skip();
    }
    this.timeout(20 * 60_000);
    const config = readSSHRuntimeConfig(CASE_MARKER);
    const workspacePath = config.workspacePaths[0];
    const sourceServerId = await readOrSeedRemoteSourceServerId(CASE_MARKER, config);
    const profileId = randomUUID();
    const credentialKey = `e2e:ssh:${randomUUID()}`;
    const workspaceIdentity = buildServerRemoteWorkspaceIdentity({
      serverId: sourceServerId,
      workspacePath,
    });

    await waitForDefaultWorkspaceReady();
    await saveCredential(CASE_MARKER, credentialKey, config.password);

    // replay Provider 必须进入本地 Personal Config，由 S4-g provisioning 在首次连接和
    // transport 恢复时同步到远端；直接写远端文件会在下一次 environment-online 同步时被覆盖。
    replayTunnel = await createSSHReplayTunnel(config, CASE_MARKER);
    const replayBaseURL = replayTunnel.baseURL;
    remoteProviderConfig = config;
    await reloadElectronSessionSafely(browser, {
      afterElectronProcessExit: async () => {
        await seedSettings({
          sourceServers: [
            {
              sourceServerId,
              displayName: "Rust Transport Drop E2E",
              preferredProfileId: profileId,
              installState: "installed",
              declaredWorkspaces: [],
            },
          ],
          connectionProfiles: [
            {
              profileId,
              sourceServerId,
              displayName: "Rust Transport Drop E2E SSH",
              transport: "ssh",
              target: {
                kind: "ssh",
                host: config.host,
                port: config.port,
                username: config.username,
                passwordCredentialKey: credentialKey,
              },
              createdAt: Date.now(),
              lifecycleState: "active",
            },
          ],
          sourceServerWorkspaceHistory: [],
        });
        await seedCustomOpenAIChatCompletionsProvider({
          baseURL: replayBaseURL,
          modelId: REMOTE_MODEL_ID,
          providerId: REMOTE_PROVIDER_ID,
          providerName: REMOTE_PROVIDER_NAME,
        });
      },
    });
    await waitForDefaultWorkspaceReady();
    await waitForProfileState(CASE_MARKER, profileId, "absent");

    await prepareV4ConversationE2E({ skipProvider: true });

    await openRemoteProfile(CASE_MARKER, profileId, sourceServerId);
    await waitForSSHDirectoryStep(CASE_MARKER);
    await selectRemoteDirectoryByPath(CASE_MARKER, workspacePath);
    await waitForWorkspaceApp(workspacePath, WORKSPACE_READY_TIMEOUT_MS);
    const before = await waitForRemoteSession(CASE_MARKER, workspacePath, workspaceIdentity);
    await waitForProfileState(CASE_MARKER, profileId, "online");
    expect(
      (await readHostProcessIdentities()).filter((host) =>
        host.name.startsWith("escode-host-remote-"),
      ),
    ).toEqual([]);

    await selectUpstreamProviderModelById(REMOTE_MODEL_ID, {
      includePlainModelFallback: false,
      providerId: REMOTE_PROVIDER_ID,
      providerName: REMOTE_PROVIDER_NAME,
    });
    await startNewV4Draft();
    const firstPrompt = `E2E_${CASE_MARKER}_A_FIRST_PROMPT`;
    const firstReply = `${CASE_MARKER}_REPLY`;
    const requestStart = await getUpstreamRequestRecordCount();
    await sendV4PromptAndWaitAccepted(
      `${firstPrompt}: 只回复 ${firstReply}`,
      firstPrompt,
      `${CASE_MARKER}: 首发没有被接受`,
    );
    const running = await waitForV4ConversationState(
      (snapshot) =>
        Boolean(snapshot.taskId && snapshot.taskId !== "draft") && snapshot.state === "streaming",
      `${CASE_MARKER}: 首发没有进入 streaming`,
      SSH_LIFECYCLE_TURN_TIMEOUT_MS,
    );
    if (!running.taskId) {
      throw new Error(`${CASE_MARKER}: 首发没有 taskId`);
    }
    await waitForUpstreamRequest(
      { lastUserMessageIncludes: [firstPrompt] },
      `${CASE_MARKER}: 首发没有到达 provider`,
      SSH_LIFECYCLE_TURN_TIMEOUT_MS,
      { afterIndex: requestStart - 1 },
    );
    await waitForV4AssistantMessageContaining(firstReply, SSH_LIFECYCLE_TURN_TIMEOUT_MS);
    await waitForV4ConversationState(
      (snapshot) =>
        snapshot.taskId === running.taskId &&
        snapshot.state === "idle" &&
        snapshot.queueCount === 0,
      `${CASE_MARKER}: 首发没有回到 idle`,
      SSH_LIFECYCLE_TURN_TIMEOUT_MS,
    );

    await killRustHostTransport(CASE_MARKER);

    // S4-e：online 链路断开先进入 retrying（2s → 4s → 8s ×3），恢复成功回到 online，
    // Renderer 消费 online 事实后自动重发 remoteSessionId，不需要手动点击重连。
    await waitForProfileState(CASE_MARKER, profileId, "retrying", 30_000);
    await waitForProfileState(CASE_MARKER, profileId, "online", 2 * 60_000);
    const after = await waitForRemoteSession(CASE_MARKER, workspacePath, workspaceIdentity);
    expect(after.remoteSessionId).not.toBe(before.remoteSessionId);
    await waitForWorkspaceApp(workspacePath, WORKSPACE_READY_TIMEOUT_MS);

    const tabs = (await readRemoteTabs()).filter(
      (tab) => tab.workspaceIdentity === workspaceIdentity,
    );
    expect(tabs).toHaveLength(1);

    // 断线前已完成的对话必须仍可读、可续发；续发请求要携带断线前历史。
    const task: SSHLifecycleTask = {
      firstPrompt,
      firstReply,
      followPrompt: `E2E_${CASE_MARKER}_A_FOLLOW_PROMPT`,
      followReply: firstReply,
      identity: workspaceIdentity,
      label: "A",
      remoteSessionId: after.remoteSessionId as string,
      taskId: running.taskId,
      workspacePath,
    };
    await sendSSHFollowTurn(task, CASE_MARKER);

    expect(
      (await readHostProcessIdentities()).filter((host) =>
        host.name.startsWith("escode-host-remote-"),
      ),
    ).toEqual([]);
  });
});
