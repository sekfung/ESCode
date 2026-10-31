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
import { seedCustomOpenAIChatCompletionsProvider } from "../helpers/custom-openai-provider-store.js";
import { reloadElectronSessionSafely } from "../helpers/e2e-electron-reload.js";
import {
  openRemoteProfile,
  saveCredential,
  selectRemoteDirectoryByPath,
  waitForProfileState,
} from "../helpers/source-server-rust-remote.js";
import {
  readHostProcessIdentities,
  waitForRemoteSession,
} from "../helpers/source-server-rust-remote-state.js";
import {
  createSSHReplayTunnel,
  readSingleSSHWorkspaceRuntimeConfig,
  runSSHCommand,
  waitForSSHDirectoryStep,
  type ReplayTunnel,
  type SSHConnectionConfig,
} from "../helpers/ssh-remote-p0.js";
import { readOrSeedRemoteSourceServerId } from "../helpers/source-server-remote-identity.js";
import { selectUpstreamProviderModelById } from "../helpers/upstream-provider.js";
import {
  prepareV4ConversationE2E,
  sendV4PromptAndWaitAccepted,
  startNewV4Draft,
  waitForV4AssistantMessageContaining,
} from "../helpers/v4-conversation.js";

const CASE_MARKER = "SOURCE_SERVER_RUST_PROVISIONING";
const MODEL_ID = "e2e-rust-provisioning-model";
const PROVIDER_ID = "e2e-rust-provisioning-provider";
const PROVIDER_NAME = "Rust Provisioning E2E";
const WORKSPACE_READY_TIMEOUT_MS = 10 * 60_000;
const TURN_TIMEOUT_MS = 2 * 60_000;
const REMOTE_PROVISIONING_STATE =
  'if [ -f "$HOME/.escode/v2/runtime/provider/provisioning.json" ]; then cat "$HOME/.escode/v2/runtime/provider/provisioning.json"; else printf \'{"records":[]}\'; fi';
const REMOTE_PERSONAL_CONFIG =
  'if [ -f "$HOME/.escode/v2/provider_config.json" ]; then cat "$HOME/.escode/v2/provider_config.json"; else printf "{}"; fi';

describe(`${CASE_MARKER}: Rust Host 首连 Provider Provisioning`, () => {
  let replayTunnel: ReplayTunnel | null = null;

  after(async () => {
    await replayTunnel?.close();
    replayTunnel = null;
    await browser.electron.restoreAllMocks();
    await clearAppData();
  });

  it("首连 barrier 把本地 Personal Provider 下发到远端并用它完成真实模型请求", async function () {
    if (!process.env.ESCODE_E2E_SSH_HOST?.trim()) {
      this.skip();
    }
    this.timeout(25 * 60_000);
    const config = readSingleSSHWorkspaceRuntimeConfig(CASE_MARKER);
    const sourceServerId = await readOrSeedRemoteSourceServerId(CASE_MARKER, config);
    const profileId = randomUUID();
    const credentialKey = `e2e:ssh:${randomUUID()}`;
    const workspaceIdentity = buildServerRemoteWorkspaceIdentity({
      serverId: sourceServerId,
      workspacePath: config.workspacePath,
    });

    // 远端 runtime 只能访问 SSH 反向转发出来的 127.0.0.1 端口，因此 Personal Provider
    // 必须写 tunnel 地址，才能证明模型请求确实由远端发起。
    replayTunnel = await createSSHReplayTunnel(config, CASE_MARKER);
    const tunnelBaseURL = replayTunnel.baseURL;

    await waitForDefaultWorkspaceReady();
    await prepareV4ConversationE2E({ skipProvider: true });
    await saveCredential(CASE_MARKER, credentialKey, config.password);
    await reloadElectronSessionSafely(browser, {
      afterElectronProcessExit: async () => {
        await seedSettings({
          sourceServers: [
            {
              sourceServerId,
              displayName: "Rust Provisioning E2E",
              preferredProfileId: profileId,
              installState: "installed",
              declaredWorkspaces: [],
            },
          ],
          connectionProfiles: [
            {
              profileId,
              sourceServerId,
              displayName: "Rust Provisioning E2E SSH",
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
        // App 未运行时直接写 Personal Config：新 Host 启动即加载同一份快照，
        // 不会出现“文件已变更但 Host 缓存仍是旧 revision”的首连失败。
        await seedCustomOpenAIChatCompletionsProvider({
          baseURL: tunnelBaseURL,
          modelId: MODEL_ID,
          providerId: PROVIDER_ID,
          providerName: PROVIDER_NAME,
        });
      },
    });
    await waitForDefaultWorkspaceReady();
    await waitForProfileState(CASE_MARKER, profileId, "absent");

    const baselineRecords = await readProvisioningRecordCount(config);
    await openRemoteProfile(CASE_MARKER, profileId, sourceServerId);
    await waitForSSHDirectoryStep(CASE_MARKER);
    await selectRemoteDirectoryByPath(CASE_MARKER, config.workspacePath);
    await waitForWorkspaceApp(config.workspacePath, WORKSPACE_READY_TIMEOUT_MS);
    await waitForProfileState(CASE_MARKER, profileId, "online");
    const session = await waitForRemoteSession(
      CASE_MARKER,
      config.workspacePath,
      workspaceIdentity,
    );
    expect(session.remoteSessionId).toBeTruthy();

    // 首连屏障在发布 online 前完成 apply，远端必须出现新的 provisioning 记录。
    const appliedRecords = await waitForProvisioningRecordGrowth(config, baselineRecords);
    expect(appliedRecords).toBeGreaterThan(baselineRecords);
    const remotePersonalConfig = await readRemotePersonalConfig(config);
    expect(remotePersonalConfig).toContain(PROVIDER_NAME);
    expect(remotePersonalConfig).toContain(MODEL_ID);

    await selectUpstreamProviderModelById(MODEL_ID, {
      includePlainModelFallback: false,
      providerId: PROVIDER_ID,
      providerName: PROVIDER_NAME,
    });
    await startNewV4Draft();
    const prompt = `${CASE_MARKER}_FIRST_PROMPT`;
    const reply = `${CASE_MARKER}_FIRST_REPLY`;
    const requestStart = await getUpstreamRequestRecordCount();
    await sendV4PromptAndWaitAccepted(
      `${prompt}: 只回复 ${reply}`,
      prompt,
      `${CASE_MARKER}: 远端首发没有被接受`,
    );
    await waitForUpstreamRequest(
      { lastUserMessageIncludes: [prompt] },
      `${CASE_MARKER}: 远端模型请求没有到达本地 Provider`,
      TURN_TIMEOUT_MS,
      { afterIndex: requestStart - 1 },
    );
    await waitForV4AssistantMessageContaining(reply, TURN_TIMEOUT_MS);

    const hostIdentities = await readHostProcessIdentities();
    expect(hostIdentities.filter((host) => host.name.startsWith("escode-host-remote-"))).toEqual(
      [],
    );
  });
});

async function readProvisioningRecordCount(config: SSHConnectionConfig): Promise<number> {
  const result = await runSSHCommand(config, REMOTE_PROVISIONING_STATE);
  if (result.code !== 0) {
    throw new Error(`${CASE_MARKER}: 读取远端 Provisioning 状态失败：${result.stderr}`);
  }
  const parsed = JSON.parse(result.stdout) as { records?: unknown };
  if (!Array.isArray(parsed.records)) {
    throw new Error(`${CASE_MARKER}: 远端 Provisioning 状态缺少 records`);
  }
  return parsed.records.length;
}

async function waitForProvisioningRecordGrowth(
  config: SSHConnectionConfig,
  baseline: number,
): Promise<number> {
  let observed = baseline;
  await browser.waitUntil(
    async () => {
      observed = await readProvisioningRecordCount(config);
      return observed > baseline;
    },
    {
      timeout: 60_000,
      interval: 500,
      timeoutMsg: `${CASE_MARKER}: 首连没有在远端完成 Provisioning`,
    },
  );
  return observed;
}

async function readRemotePersonalConfig(config: SSHConnectionConfig): Promise<string> {
  const result = await runSSHCommand(config, REMOTE_PERSONAL_CONFIG);
  if (result.code !== 0) {
    throw new Error(`${CASE_MARKER}: 读取远端 Personal Config 失败：${result.stderr}`);
  }
  return result.stdout;
}
