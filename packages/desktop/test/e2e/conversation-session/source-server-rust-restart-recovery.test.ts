import { randomUUID } from "node:crypto";
import { TID_PROJECT_SECTION, buildServerRemoteWorkspaceIdentity } from "@escode/shared";
import {
  clearAppData,
  seedSettings,
  waitForDefaultWorkspaceReady,
  waitForWorkspaceApp,
} from "../helpers/desktop-app.js";
import { reloadElectronSessionSafely } from "../helpers/e2e-electron-reload.js";
import {
  openRemoteProfile,
  saveCredential,
  selectRemoteDirectoryByPath,
  waitForProfileState,
} from "../helpers/source-server-rust-remote.js";
import {
  readHostProcessIdentities,
  readRemoteTabs,
  waitForPersistedActiveRemoteWorkspace,
  waitForPersistedSourceLastOnlineAt,
  waitForRemoteSession,
  waitForRestoredRemoteTab,
} from "../helpers/source-server-rust-remote-state.js";
import { readSSHRuntimeConfig, waitForSSHDirectoryStep } from "../helpers/ssh-remote-p0.js";
import { readOrSeedRemoteSourceServerId } from "../helpers/source-server-remote-identity.js";

const CASE_MARKER = "SOURCE_SERVER_RUST_RESTART_RECOVERY";
const WORKSPACE_READY_TIMEOUT_MS = 2 * 60_000;

describe(`${CASE_MARKER}: Rust Host 远端 workspace 重启恢复`, () => {
  after(async () => {
    await browser.electron.restoreAllMocks();
    await clearAppData();
  });

  it("App 重启后按 lastOnlineAt 自动恢复远端 tab，不启动 per-window Node Host", async function () {
    if (!process.env.ESCODE_E2E_SSH_HOST?.trim()) {
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

    await reloadElectronSessionSafely(browser, {
      afterElectronProcessExit: async () => {
        await seedSettings({
          sourceServers: [
            {
              sourceServerId,
              displayName: "Rust Restart E2E",
              preferredProfileId: profileId,
              installState: "installed",
              declaredWorkspaces: [],
            },
          ],
          connectionProfiles: [
            {
              profileId,
              sourceServerId,
              displayName: "Rust Restart E2E SSH",
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
      },
    });
    await waitForDefaultWorkspaceReady();
    await waitForProfileState(CASE_MARKER, profileId, "absent");

    await openRemoteProfile(CASE_MARKER, profileId, sourceServerId);
    await waitForSSHDirectoryStep(CASE_MARKER);
    await selectRemoteDirectoryByPath(CASE_MARKER, workspacePath);
    await waitForWorkspaceApp(workspacePath, WORKSPACE_READY_TIMEOUT_MS);
    const session = await waitForRemoteSession(CASE_MARKER, workspacePath, workspaceIdentity);
    if (!session.remoteSessionId) {
      throw new Error(`${CASE_MARKER}: 远端 session 缺少 remoteSessionId`);
    }
    await waitForProfileState(CASE_MARKER, profileId, "online");

    // 重启事实必须来自 settings：活动远端 workspace、脱敏 target 与 Source 级 lastOnlineAt。
    const persisted = await waitForPersistedActiveRemoteWorkspace(CASE_MARKER, workspaceIdentity);
    const lastOnlineAt = await waitForPersistedSourceLastOnlineAt(CASE_MARKER, sourceServerId);
    expect(lastOnlineAt).toBeGreaterThan(0);
    expect(persisted.workspacePath).toBe(workspacePath);
    expect(persisted.target.kind).toBe("ssh");
    expect(JSON.stringify(persisted.target)).not.toContain(config.password);

    const tabsBeforeRestart = await readRemoteTabs();
    expect(
      tabsBeforeRestart.filter((tab) => tab.workspaceIdentity === workspaceIdentity),
    ).toHaveLength(1);
    const hostsBeforeRestart = await readHostProcessIdentities();
    expect(
      hostsBeforeRestart.filter((host) => host.name.startsWith("escode-host-remote-")),
    ).toEqual([]);

    // 不重新 seed：重启只允许从磁盘恢复绑定与断连 tab。
    await reloadElectronSessionSafely(browser);
    await waitForAppShell();

    const restoredTab = await waitForRestoredRemoteTab(
      CASE_MARKER,
      workspacePath,
      workspaceIdentity,
    );
    expect(restoredTab.workspaceIdentity).toBe(workspaceIdentity);

    // S4-e：Host 对近 7 天 lastOnlineAt 的 Source 静默自动连接；Renderer 只消费 online
    // 事实并给已打开的断连 tab 换发 remoteSessionId，不额外发起第二条启动恢复。
    await waitForProfileState(CASE_MARKER, profileId, "online", 2 * 60_000);
    const reconnected = await waitForRemoteSession(CASE_MARKER, workspacePath, workspaceIdentity);
    expect(reconnected.remoteSessionId).not.toBe(session.remoteSessionId);
    await waitForWorkspaceApp(workspacePath, WORKSPACE_READY_TIMEOUT_MS);

    const tabsAfterReconnect = await readRemoteTabs();
    expect(
      tabsAfterReconnect.filter((tab) => tab.workspaceIdentity === workspaceIdentity),
    ).toHaveLength(1);
    const hostsAfterReconnect = await readHostProcessIdentities();
    expect(
      hostsAfterReconnect.filter((host) => host.name.startsWith("escode-host-remote-")),
    ).toEqual([]);
  });
});

async function waitForAppShell(): Promise<void> {
  await browser.waitUntil(
    () =>
      browser.execute(
        (testId) => Boolean(document.querySelector(`[data-testid="${testId}"]`)),
        TID_PROJECT_SECTION,
      ),
    {
      timeout: 60_000,
      interval: 500,
      timeoutMsg: `${CASE_MARKER}: 重启后主界面没有恢复`,
    },
  );
}
