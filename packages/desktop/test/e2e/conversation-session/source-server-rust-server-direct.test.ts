import { randomUUID } from "node:crypto";
import {
  TID_PROJECT_SECTION,
  buildServerRemoteWorkspaceIdentity,
  type ConnectionProfile,
} from "@escode/shared";
import {
  clearAppData,
  getAllWindowInfo,
  seedSettings,
  waitForWorkspaceApp,
  waitForTestIdByDom,
} from "../helpers/desktop-app.js";
import { reloadElectronSessionSafely } from "../helpers/e2e-electron-reload.js";
import { startLocalESCodeServer, type LocalESCodeServer } from "../helpers/local-escode-server.js";
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
  waitForProfileStateNot,
} from "../helpers/source-server-rust-remote.js";
import {
  readHostProcessIdentities,
  waitForRemoteSession,
} from "../helpers/source-server-rust-remote-state.js";
import { waitForSSHDirectoryStep } from "../helpers/ssh-remote-p0.js";
import { prepareV4ConversationE2E } from "../helpers/v4-conversation.js";

const CASE_MARKER = "SOURCE_SERVER_RUST_SERVER_DIRECT";
const WORKSPACE_READY_TIMEOUT_MS = 3 * 60_000;

describe(`${CASE_MARKER}: Rust Host 直连真实 escode-server`, () => {
  let server: LocalESCodeServer | undefined;
  let profileId: string;
  let credentialKey: string;
  /** 失败时把卡住的阶段写进错误里：WDIO 的裸 `Timeout` 不带调用点，跨平台排查只能靠阶段标记。 */
  let stage = "before";
  /** 同时打到 worker 日志：裸 `Timeout` 有时绕过用例内的 catch，只能看最后打印的阶段。 */
  const mark = (next: string) => {
    stage = next;
    console.log(`[${CASE_MARKER}] stage=${next}`);
  };

  before(async () => {
    server = await startLocalESCodeServer();
  });

  after(async () => {
    await browser.electron.restoreAllMocks();
    // before 失败时 server 未创建；旧写法会抛 `reading 'stop'` 掩盖真实的启动错误。
    await server?.stop();
    await clearAppData();
  });

  it("真实 server 建链 → App 重启恢复 → 断线原位重连 → 双窗口共享，且无 Node 远程进程", async function () {
    this.timeout(30 * 60_000);
    try {
      await runServerDirectScenario();
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      throw new Error(`${CASE_MARKER}: 阶段 ${stage} 失败：${message}`);
    }
  });

  async function runServerDirectScenario(): Promise<void> {
    if (!server) throw new Error("server fixture 未初始化");
    profileId = randomUUID();
    credentialKey = `e2e:server:${randomUUID()}`;
    const workspaceIdentity = buildServerRemoteWorkspaceIdentity({
      serverId: server.serverId,
      workspacePath: server.workspacePath,
    });
    mark("seed-profile");
    await prepareV4ConversationE2E({ skipProvider: true });
    await saveCredential(CASE_MARKER, credentialKey, server.token);
    await seedServerProfile(server, profileId, credentialKey, workspaceIdentity);
    // 重启后的种子 Profile 会让侧栏进入 Source Server 视图；这里只等应用壳，不要求本地工作区条目。
    await waitForTestIdByDom(TID_PROJECT_SECTION, {
      timeout: 60_000,
      timeoutMsg: `${CASE_MARKER}: 重启后没有恢复应用壳`,
    });

    // §12.6.7-4：Rust Host 用持久 Profile + 凭据服务建立真实 HTTP/WS 链路。
    mark("open-profile");
    await openRemoteProfile(CASE_MARKER, profileId, server.serverId);
    mark("select-directory");
    await waitForSSHDirectoryStep(CASE_MARKER);
    await selectRemoteDirectoryByPath(CASE_MARKER, server.workspacePath);
    mark("wait-workspace-ready");
    await waitForWorkspaceApp(server.workspacePath, WORKSPACE_READY_TIMEOUT_MS);
    await waitForProfileState(CASE_MARKER, profileId, "online");
    mark("wait-remote-session");
    const firstSession = await waitForRemoteSession(
      CASE_MARKER,
      server.workspacePath,
      workspaceIdentity,
    );
    expect(firstSession.remoteSessionId).toBeTruthy();

    // App 重启后从持久 Profile 恢复 tab，并由 Rust Host 重新建链。
    mark("restart-recovery");
    await reloadElectronSessionSafely(browser);
    await waitForWorkspaceApp(server.workspacePath, WORKSPACE_READY_TIMEOUT_MS);
    await waitForProfileState(CASE_MARKER, profileId, "online");
    // 重启会换掉 Rust Host 进程，所以基线要在重启之后取。
    const hostsAfterRestart = await readHostProcessIdentities();
    expect(hostsAfterRestart.filter((host) => host.name.startsWith("escode-host-remote-"))).toEqual(
      [],
    );

    // 断线 → 原位重连：停掉真实服务端，等到状态离开 online，再按同端口/同身份启动。
    mark("disconnect-reconnect");
    await server.stopProcess();
    await waitForProfileStateNot(CASE_MARKER, profileId, "online", 2 * 60_000);
    await server.startProcess();
    await waitForProfileState(CASE_MARKER, profileId, "online", 3 * 60_000);

    // §12.6.7-5：第二个窗口恢复同一远端 workspace，共享 Rust Host 实例。
    mark("second-window");
    await openSecondMainWindow(CASE_MARKER);
    await waitForSecondWindowAppShell(CASE_MARKER);
    await waitForSecondWindowRemoteTab(CASE_MARKER, server.workspacePath);
    const windows = await getAllWindowInfo();
    expect(windows.filter((window) => window.url.includes("index.html")).length).toBeGreaterThan(1);

    // §12.6.7-6：远程链路全部由 Rust Host 承载，不出现 Node UtilityProcess 远程进程。
    // 每个窗口各有一个 window-scoped Local Host，进程 pid 会不同；这里断言的是「没有任何
    // Node UtilityProcess 远程 Host」，而不是 pid 集合相等。
    mark("assert-zero-node-host");
    const hosts = await readHostProcessIdentities();
    expect(hosts.filter((host) => host.name.startsWith("escode-host-remote-"))).toEqual([]);
    mark("done");
  }
});

async function seedServerProfile(
  server: LocalESCodeServer,
  profileId: string,
  credentialKey: string,
  workspaceIdentity: string,
): Promise<void> {
  const profile: Extract<ConnectionProfile, { transport: "server" }> = {
    profileId,
    sourceServerId: server.serverId,
    displayName: "Local E2E Server",
    transport: "server",
    target: { kind: "server", url: server.url, tokenCredentialKey: credentialKey },
    createdAt: Date.now(),
    lifecycleState: "active",
  };
  // 只 seed Profile + 凭据；连接由 Rust Host 在打开项目时真实建立，不是测试内假连接。
  await reloadElectronSessionSafely(browser, {
    afterElectronProcessExit: async () => {
      await seedSettings({
        sourceServers: [
          {
            sourceServerId: server.serverId,
            displayName: "Local E2E Server",
            preferredProfileId: profileId,
            installState: "installed",
            declaredWorkspaces: [
              { workspacePath: server.workspacePath, workspaceIdentity, label: "workspace" },
            ],
          },
        ],
        connectionProfiles: [profile],
        sourceServerWorkspaceHistory: [],
      });
    },
  });
}
