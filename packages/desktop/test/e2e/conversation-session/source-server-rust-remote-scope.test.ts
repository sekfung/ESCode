import { randomUUID } from "node:crypto";
import { buildServerRemoteWorkspaceIdentity } from "@escode/shared";
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
  waitForRemoteSession,
} from "../helpers/source-server-rust-remote-state.js";
import { readSSHRuntimeConfig, waitForSSHDirectoryStep } from "../helpers/ssh-remote-p0.js";
import { readOrSeedRemoteSourceServerId } from "../helpers/source-server-remote-identity.js";

const CASE_MARKER = "SOURCE_SERVER_RUST_REMOTE_SCOPE";
const WORKSPACE_READY_TIMEOUT_MS = 2 * 60_000;

describe(`${CASE_MARKER}: Rust Host 远端 workspace`, () => {
  after(async () => {
    await browser.electron.restoreAllMocks();
    await clearAppData();
  });

  it("经 Connection Profile 打开远端 workspace 由 Rust Host 服务，不启动 per-window Node Host", async function () {
    if (!process.env.ESCODE_E2E_SSH_HOST?.trim()) {
      this.skip();
    }
    this.timeout(20 * 60_000);
    const config = readSSHRuntimeConfig(CASE_MARKER);
    const [workspaceA, workspaceB] = config.workspacePaths;
    const sourceServerId = await readOrSeedRemoteSourceServerId(CASE_MARKER, config);
    const profileId = randomUUID();
    const credentialKey = `e2e:ssh:${randomUUID()}`;
    const workspaceIdentity = buildServerRemoteWorkspaceIdentity({
      serverId: sourceServerId,
      workspacePath: workspaceA,
    });
    const workspaceIdentityB = buildServerRemoteWorkspaceIdentity({
      serverId: sourceServerId,
      workspacePath: workspaceB,
    });

    await waitForDefaultWorkspaceReady();
    await saveCredential(CASE_MARKER, credentialKey, config.password);

    await reloadElectronSessionSafely(browser, {
      afterElectronProcessExit: async () => {
        await seedSettings({
          sourceServers: [
            {
              sourceServerId,
              displayName: "Rust Remote E2E",
              preferredProfileId: profileId,
              installState: "installed",
              declaredWorkspaces: [],
            },
          ],
          connectionProfiles: [
            {
              profileId,
              sourceServerId,
              displayName: "Rust Remote E2E SSH",
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
    await selectRemoteDirectoryByPath(CASE_MARKER, workspaceA);
    await waitForWorkspaceApp(workspaceA, WORKSPACE_READY_TIMEOUT_MS);

    const session = await waitForRemoteSession(CASE_MARKER, workspaceA, workspaceIdentity);
    expect(session.workspaceIdentity).toBe(workspaceIdentity);
    if (!session.remoteSessionId) {
      throw new Error(`${CASE_MARKER}: 远端 session 缺少 remoteSessionId`);
    }
    // 只有 Rust Host 的连接管理器会写 online；Node Host 远程路径没有这个状态投影。
    await waitForProfileState(CASE_MARKER, profileId, "online");

    await openRemoteProfile(CASE_MARKER, profileId, sourceServerId);
    await waitForSSHDirectoryStep(CASE_MARKER);
    await selectRemoteDirectoryByPath(CASE_MARKER, workspaceB);
    await waitForWorkspaceApp(workspaceB, WORKSPACE_READY_TIMEOUT_MS);
    const sessionB = await waitForRemoteSession(CASE_MARKER, workspaceB, workspaceIdentityB);
    expect(sessionB.workspaceIdentity).toBe(workspaceIdentityB);
    expect(sessionB.remoteSessionId).not.toBe(session.remoteSessionId);

    const hostIdentities = await readHostProcessIdentities();
    expect(hostIdentities.filter((host) => host.name.startsWith("escode-host-remote-"))).toEqual(
      [],
    );
  });
});
