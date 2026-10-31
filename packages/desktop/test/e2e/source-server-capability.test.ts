import { TID_COMPOSER_WORKSPACE_TRIGGER } from "@escode/shared";
import { clearAppData, waitForDefaultWorkspaceReady } from "./helpers/desktop-app.js";
import { startNewV4Draft } from "./helpers/v4-conversation.js";
import {
  completeSSHDeviceConnection,
  fillSSHPasswordSettings,
  openSSHDevicePicker,
  readSingleSSHWorkspaceRuntimeConfig,
} from "./helpers/ssh-remote-p0.js";

const CASE_MARKER = "SOURCE_SERVER_CAPABILITY";

interface CapabilitySnapshot {
  bridgeMissing: boolean;
  baseBound: boolean;
  sessionId: string | null;
  managementBound: boolean;
}

describe(`${CASE_MARKER}: Source Server 能力绑定`, () => {
  after(async () => {
    await browser.electron.restoreAllMocks();
    await clearAppData();
  });

  it("本地窗口在 Initialize 后绑定 Source Server 管理服务", async () => {
    await waitForDefaultWorkspaceReady();
    await browser.waitUntil(async () => (await readCapabilitySnapshot("")).baseBound, {
      timeout: 30_000,
      interval: 250,
      timeoutMsg: `${CASE_MARKER}: 本地 Rust Host 没有广播 source-server-management`,
    });

    const snapshot = await readCapabilitySnapshot("");
    expect(snapshot.bridgeMissing).toBe(false);
    expect(snapshot.baseBound).toBe(true);
    // 本地窗口尚未打开远端 workspace，不能出现任何 remote session。
    expect(snapshot.sessionId).toBeNull();
  });

  it("SSH 远端窗口不绑定 Source Server 管理服务", async function () {
    if (!process.env.ESCODE_E2E_SSH_HOST?.trim()) {
      this.skip();
    }
    this.timeout(15 * 60_000);
    const config = readSingleSSHWorkspaceRuntimeConfig(CASE_MARKER);

    await waitForDefaultWorkspaceReady();
    await startNewV4Draft();
    await waitForDraftWorkspaceSettled();
    await openSSHDevicePicker(CASE_MARKER);
    await fillSSHPasswordSettings(config);
    // S5-e 后「添加设备」连接成功即关闭向导，由项目选择器承接目录并创建项目。
    await completeSSHDeviceConnection({
      caseMarker: CASE_MARKER,
      workspacePath: config.workspacePath,
    });
    await waitForRemoteTabPath(config.workspacePath);
    await browser.pause(500);

    let latest: CapabilitySnapshot | null = null;
    await browser.waitUntil(
      async () => {
        latest = await readCapabilitySnapshot(config.workspacePath);
        return latest.sessionId !== null && latest.managementBound === false;
      },
      {
        timeout: 30_000,
        interval: 500,
        timeoutMsg: `${CASE_MARKER}: 远端 session 能力不正确 latest=${JSON.stringify(latest)}`,
      },
    );

    expect(latest).not.toBeNull();
    expect(latest!.managementBound).toBe(false);
    // 本地基础服务仍由 Rust Host 提供；远端 scoped port 也由 App 级 Rust Host 提供，不创建 per-window Node Host。
    expect(latest!.baseBound).toBe(true);
  });
});

async function waitForRemoteTabPath(workspacePath: string): Promise<void> {
  await browser.waitUntil(
    async () => {
      try {
        return await browser.execute((expectedPath) => {
          const state = (
            window as Window & {
              __escodeTabStoreE2E?: {
                getState?: () => {
                  activeWorkspacePath?: string | null;
                  tabs?: Array<{
                    workspacePath?: string;
                    remoteSessionId?: string;
                  }>;
                };
              };
            }
          ).__escodeTabStoreE2E?.getState?.();
          const tab = state?.tabs?.find((candidate) => candidate.workspacePath === expectedPath);
          return Boolean(state?.activeWorkspacePath === expectedPath && tab?.remoteSessionId);
        }, workspacePath);
      } catch {
        return false;
      }
    },
    {
      timeout: 60_000,
      interval: 500,
      timeoutMsg: `${CASE_MARKER}: 远端 tab 没有收敛到 ${workspacePath}`,
    },
  );
}

function readCapabilitySnapshot(workspacePath: string): Promise<CapabilitySnapshot> {
  return browser.execute((expectedPath) => {
    type ServiceAccessor = {
      sourceServerManagementService?: unknown;
    };
    const store = (
      window as Window & {
        __remoteWorkspaceSessionStoreE2E?: {
          getState?: () => {
            baseServices?: ServiceAccessor | null;
            sessionIdByWorkspacePath?: Record<string, string>;
            sessionsById?: Record<
              string,
              {
                sessionId: string;
                services?: ServiceAccessor;
              }
            >;
          };
        };
      }
    ).__remoteWorkspaceSessionStoreE2E;
    if (!store?.getState) {
      return {
        bridgeMissing: true,
        baseBound: false,
        sessionId: null,
        managementBound: false,
      };
    }
    const state = store.getState();
    const sessionId = expectedPath
      ? (state.sessionIdByWorkspacePath?.[expectedPath] ?? null)
      : null;
    const session = sessionId ? state.sessionsById?.[sessionId] : undefined;
    return {
      bridgeMissing: false,
      baseBound: Boolean(state.baseServices?.sourceServerManagementService),
      sessionId,
      managementBound: Boolean(session?.services?.sourceServerManagementService),
    };
  }, workspacePath);
}

async function waitForDraftWorkspaceSettled(): Promise<void> {
  let previousKey: string | null = null;
  let stablePolls = 0;
  await browser.waitUntil(
    async () => {
      const snapshot = await browser.execute((triggerTestId) => {
        const state = (
          window as Window & {
            __escodeTabStoreE2E?: {
              getState?: () => {
                activeTabId?: string | null;
                activeWorkspacePath?: string | null;
              };
            };
          }
        ).__escodeTabStoreE2E?.getState?.();
        const visibleTriggerCount = Array.from(
          document.querySelectorAll<HTMLElement>(`[data-testid="${triggerTestId}"]`),
        ).filter((element) => element.getClientRects().length > 0).length;
        return {
          activeTabId: state?.activeTabId ?? null,
          activeWorkspacePath: state?.activeWorkspacePath ?? null,
          visibleTriggerCount,
        };
      }, TID_COMPOSER_WORKSPACE_TRIGGER);
      const key = JSON.stringify(snapshot);
      stablePolls = key === previousKey ? stablePolls + 1 : 0;
      previousKey = key;
      return Boolean(
        snapshot.activeTabId &&
        snapshot.activeWorkspacePath &&
        snapshot.visibleTriggerCount === 1 &&
        stablePolls >= 2,
      );
    },
    {
      timeout: 15_000,
      interval: 100,
      timeoutMsg: `${CASE_MARKER}: 新任务草稿的 workspace 没有稳定`,
    },
  );
}
