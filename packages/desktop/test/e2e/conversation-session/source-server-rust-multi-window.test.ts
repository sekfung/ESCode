import { randomUUID } from "node:crypto";
import { buildServerRemoteWorkspaceIdentity } from "@escode/shared";
import {
  clearAppData,
  getAllWindowInfo,
  seedSettings,
  waitForDefaultWorkspaceReady,
  waitForWorkspaceApp,
} from "../helpers/desktop-app.js";
import { reloadElectronSessionSafely } from "../helpers/e2e-electron-reload.js";
import {
  openSecondMainWindow,
  waitForSecondWindowAppShell,
  waitForSecondWindowRemoteTab,
} from "../helpers/second-main-window.js";
import {
  openRemoteProfile,
  saveCredential,
  selectRemoteDirectoryByPath,
  waitForProfileState,
} from "../helpers/source-server-rust-remote.js";
import {
  readHostProcessIdentities,
  waitForPersistedActiveRemoteWorkspace,
  waitForRemoteSession,
} from "../helpers/source-server-rust-remote-state.js";
import { readSSHRuntimeConfig, waitForSSHDirectoryStep } from "../helpers/ssh-remote-p0.js";
import { readOrSeedRemoteSourceServerId } from "../helpers/source-server-remote-identity.js";

const CASE_MARKER = "SOURCE_SERVER_RUST_MULTI_WINDOW";
const WORKSPACE_READY_TIMEOUT_MS = 2 * 60_000;

describe(`${CASE_MARKER}: Rust Host 远端 workspace 多窗口`, () => {
  after(async () => {
    await browser.electron.restoreAllMocks();
    await clearAppData();
  });

  it("同一远端 workspace 的两个窗口共享 Host 实例，各自持有独立 attachment", async function () {
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
              displayName: "Rust Multi Window E2E",
              preferredProfileId: profileId,
              installState: "installed",
              declaredWorkspaces: [],
            },
          ],
          connectionProfiles: [
            {
              profileId,
              sourceServerId,
              displayName: "Rust Multi Window E2E SSH",
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
    const firstWindowSession = await waitForRemoteSession(
      CASE_MARKER,
      workspacePath,
      workspaceIdentity,
    );
    await waitForProfileState(CASE_MARKER, profileId, "online");

    // 第二个窗口从同一份 settings 恢复；必须等首个窗口把活动远端 workspace 落盘。
    await waitForPersistedActiveRemoteWorkspace(CASE_MARKER, workspaceIdentity);
    await openSecondMainWindow(CASE_MARKER);
    await waitForSecondWindowAppShell(CASE_MARKER);
    // 第二个窗口从 settings 恢复同一远端 tab；profile 已 online，启动恢复会直接 attach。
    await waitForSecondWindowRemoteTab(CASE_MARKER, workspacePath);
    const secondWindowSession = await waitForRemoteSession(
      CASE_MARKER,
      workspacePath,
      workspaceIdentity,
    );
    expect(secondWindowSession.remoteSessionId).not.toBe(firstWindowSession.remoteSessionId);

    const windows = await getAllWindowInfo();
    expect(windows.filter((window) => window.url.includes("index.html")).length).toBeGreaterThan(1);

    const hosts = await readHostProcessIdentities();
    expect(hosts.filter((host) => host.name.startsWith("escode-host-remote-"))).toEqual([]);
  });
});
