import { parseRemoteWorkspaceIdentity, type ConnectionProfile } from "@escode/shared";
import { clearAppData, readSettings } from "../helpers/desktop-app.js";
import { waitForProfileState } from "../helpers/source-server-rust-remote.js";
import { readHostProcessIdentities } from "../helpers/source-server-rust-remote-state.js";
import { connectSSHWorkspace, readSSHRuntimeConfig } from "../helpers/ssh-remote-p0.js";
import { prepareV4ConversationE2E } from "../helpers/v4-conversation.js";

const CASE_MARKER = "SOURCE_SERVER_RUST_FIRST_CONNECT";

type ResolvedSSHProfile = Extract<ConnectionProfile, { transport: "ssh" }> & {
  sourceServerId: string;
};

describe(`${CASE_MARKER}: Rust Host 直接首连`, () => {
  after(async () => {
    await browser.electron.restoreAllMocks();
    await clearAppData();
  });

  it("无 profileId 的 SSH 首连由 Rust Host 创建 Profile 并回填 Source identity", async function () {
    if (!process.env.ESCODE_E2E_SSH_HOST?.trim()) {
      this.skip();
    }
    this.timeout(20 * 60_000);
    const config = readSSHRuntimeConfig(CASE_MARKER);
    const workspacePath = config.workspacePaths[0];

    await prepareV4ConversationE2E({ skipProvider: true });
    const initialHosts = await readHostProcessIdentities();
    const tab = await connectSSHWorkspace({
      caseMarker: CASE_MARKER,
      config,
      workspacePath,
    });

    expect(tab.secretPersistedInTab).toBe(false);
    const parsedIdentity = parseRemoteWorkspaceIdentity(tab.workspaceIdentity ?? "");
    if (parsedIdentity?.kind !== "server" || !parsedIdentity.sourceServerId) {
      throw new Error(`${CASE_MARKER}: 直接首连没有收敛到 Source Server identity`);
    }
    expect(parsedIdentity.workspacePath).toBe(workspacePath.replace(/\/+$/, "") || "/");

    const profile = await waitForResolvedProfile(CASE_MARKER, config);
    expect(profile.sourceServerId).toBe(parsedIdentity.sourceServerId);
    expect(profile.target).not.toHaveProperty("password");
    expect(profile.target).not.toHaveProperty("privateKeyPassphrase");
    expect(profile.target.passwordCredentialKey).toBe(
      `connection-profile:${profile.profileId}:password`,
    );

    await waitForPreferredProfile(CASE_MARKER, profile.sourceServerId, profile.profileId);

    // 只把连接事实写入 Rust Host registry；Main 不应在认证/首连阶段懒创建 Node Host。
    await waitForProfileState(CASE_MARKER, profile.profileId, "online");
    const hosts = await readHostProcessIdentities();
    expect(hosts).toEqual(expect.arrayContaining(initialHosts));
    expect(hosts.filter((host) => host.name.startsWith("escode-host-remote-"))).toEqual([]);
  });
});

async function waitForResolvedProfile(
  caseMarker: string,
  config: ReturnType<typeof readSSHRuntimeConfig>,
): Promise<ResolvedSSHProfile> {
  let latest: ResolvedSSHProfile | undefined;
  await browser.waitUntil(
    async () => {
      const settings = await readSettings();
      latest = settings.connectionProfiles?.find(
        (profile): profile is ResolvedSSHProfile =>
          profile.transport === "ssh" &&
          typeof profile.sourceServerId === "string" &&
          profile.target.host.toLowerCase() === config.host.toLowerCase() &&
          (profile.target.port ?? 22) === config.port &&
          profile.target.username === config.username,
      );
      return Boolean(latest?.profileId && latest.sourceServerId);
    },
    {
      timeout: 60_000,
      interval: 500,
      timeoutMsg: `${caseMarker}: Rust Host 没有持久化首连 Profile/sourceServerId`,
    },
  );
  if (!latest) {
    throw new Error(`${caseMarker}: 首连 Profile 缺少 sourceServerId`);
  }
  return latest;
}

async function waitForPreferredProfile(
  caseMarker: string,
  sourceServerId: string,
  profileId: string,
): Promise<void> {
  let latest: string | undefined;
  await browser.waitUntil(
    async () => {
      const settings = await readSettings();
      latest = settings.sourceServers?.find(
        (candidate) => candidate.sourceServerId === sourceServerId,
      )?.preferredProfileId;
      return latest === profileId;
    },
    {
      timeout: 30_000,
      interval: 500,
      timeoutMsg: `${caseMarker}: Source ${sourceServerId} 没有绑定首连 Profile ${profileId}，latest=${String(latest)}`,
    },
  );
}
