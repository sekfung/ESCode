import { randomUUID } from "node:crypto";
import { TID_PROJECT_SECTION, buildServerRemoteWorkspaceIdentity } from "@escode/shared";
import {
  clearAppData,
  getAllWindowInfo,
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
    await openSecondMainWindow();
    await waitForAppShell();
    // 第二个窗口从 settings 恢复同一远端 tab；profile 已 online，启动恢复会直接 attach。
    await waitForSecondWindowRemoteTab(workspacePath);
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

/**
 * 产品目前没有“新建窗口”菜单；macOS 的 activate 是创建主窗口的真实入口。
 * 测试临时把已有窗口视为不存在，走同一 coordinator/loadWindow 路径创建第二个窗口，
 * 而不是手写 BrowserWindow 配置绕过应用初始化。
 */
async function openSecondMainWindow(): Promise<void> {
  const handles = await browser.getWindowHandles();
  await browser.electron.execute(async (electron) => {
    const originalWindows = electron.BrowserWindow.getAllWindows;
    const created = new Promise<void>((resolve) => {
      electron.app.once("browser-window-created", () => {
        electron.BrowserWindow.getAllWindows = originalWindows;
        resolve();
      });
    });
    electron.BrowserWindow.getAllWindows = () => [];
    try {
      electron.app.emit("activate");
      await created;
    } finally {
      electron.BrowserWindow.getAllWindows = originalWindows;
    }
  });

  let handle = "";
  await browser.waitUntil(
    async () => {
      handle =
        (await browser.getWindowHandles()).find((candidate) => !handles.includes(candidate)) ?? "";
      return Boolean(handle);
    },
    {
      timeout: 30_000,
      interval: 250,
      timeoutMsg: `${CASE_MARKER}: 第二个主窗口没有出现`,
    },
  );
  await browser.switchToWindow(handle);
}

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
      timeoutMsg: `${CASE_MARKER}: 第二个窗口没有恢复主界面`,
    },
  );
}

async function waitForSecondWindowRemoteTab(workspacePath: string): Promise<void> {
  try {
    await browser.waitUntil(
      () =>
        browser.execute(
          (expectedPath) =>
            Boolean(
              (
                window as Window & {
                  __escodeTabStoreE2E?: {
                    getState?: () => {
                      tabs?: Array<{ workspacePath?: string }>;
                    };
                  };
                }
              ).__escodeTabStoreE2E
                ?.getState?.()
                .tabs?.some((tab) => tab.workspacePath === expectedPath),
            ),
          workspacePath,
        ),
      {
        timeout: 30_000,
        interval: 500,
        timeoutMsg: `${CASE_MARKER}: 第二个窗口没有恢复远端 tab`,
      },
    );
  } catch (error) {
    const diagnostics = await browser.execute(() => {
      const tabState = (
        window as Window & {
          __escodeTabStoreE2E?: { getState?: () => unknown };
        }
      ).__escodeTabStoreE2E?.getState?.();
      const sessionState = (
        window as Window & {
          __remoteWorkspaceSessionStoreE2E?: { getState?: () => unknown };
        }
      ).__remoteWorkspaceSessionStoreE2E?.getState?.();
      return {
        href: window.location.href,
        sessionState,
        tabState,
      };
    });
    throw new Error(
      `${error instanceof Error ? error.message : String(error)} ${JSON.stringify(diagnostics)}`,
      { cause: error },
    );
  }
}
