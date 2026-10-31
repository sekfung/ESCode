import { readSettings } from "./desktop-app.js";
import {
  isMainRendererUrl,
  isResourceManagerUrl,
  listHostProcessIdentities,
  openResourceManager,
  switchToElectronRendererTarget,
  waitForResourceManagerReady,
} from "./resource-manager.js";

export interface RemoteSessionSnapshot {
  remoteSessionId: string | null;
  sessionId: string;
  workspaceIdentity: string | null;
  workspacePath: string | null;
}

export interface RemoteTabSnapshot {
  label: string | null;
  remoteSessionId: string | null;
  workspaceIdentity: string | null;
  workspacePath: string | null;
}

export interface PersistedRemoteWorkspace {
  target: { kind: string };
  workspacePath: string;
}

export async function waitForRemoteSession(
  caseMarker: string,
  workspacePath: string,
  expectedIdentity: string,
): Promise<RemoteSessionSnapshot> {
  let latest: RemoteSessionSnapshot | null = null;
  await browser.waitUntil(
    async () => {
      latest = await browser.execute((expectedPath) => {
        const tabStore = (
          window as Window & {
            __escodeTabStoreE2E?: {
              getState?: () => {
                tabs?: Array<{
                  remoteSessionId?: string;
                  workspaceIdentity?: string;
                  workspacePath?: string;
                }>;
              };
            };
          }
        ).__escodeTabStoreE2E;
        const tab = tabStore
          ?.getState?.()
          .tabs?.find((candidate) => candidate.workspacePath === expectedPath);
        const sessionStore = (
          window as Window & {
            __remoteWorkspaceSessionStoreE2E?: {
              getState?: () => {
                sessionsById?: Record<
                  string,
                  {
                    sessionId: string;
                    sourceServerId?: string;
                    workspaceIdentity?: string;
                    workspacePath?: string;
                  }
                >;
              };
            };
          }
        ).__remoteWorkspaceSessionStoreE2E;
        const session = tab?.remoteSessionId
          ? sessionStore?.getState?.().sessionsById?.[tab.remoteSessionId]
          : undefined;
        if (!tab?.remoteSessionId || !session) {
          return null;
        }
        return {
          remoteSessionId: tab.remoteSessionId,
          sessionId: session.sessionId,
          workspaceIdentity: session.workspaceIdentity ?? tab.workspaceIdentity ?? null,
          workspacePath: session.workspacePath ?? tab.workspacePath ?? null,
        };
      }, workspacePath);
      return Boolean(latest?.remoteSessionId && latest.workspaceIdentity === expectedIdentity);
    },
    {
      timeout: 60_000,
      interval: 500,
      timeoutMsg: `${caseMarker}: 远端 session 没有收敛到 ${workspacePath}`,
    },
  );
  if (!latest) throw new Error(`${caseMarker}: 远端 session 快照缺失`);
  return latest;
}

/** 重启后只恢复断连 tab：有稳定 identity，但没有 remoteSessionId。 */
export async function waitForRestoredRemoteTab(
  caseMarker: string,
  workspacePath: string,
  expectedIdentity: string,
): Promise<RemoteTabSnapshot> {
  let latest: RemoteTabSnapshot | null = null;
  await browser.waitUntil(
    async () => {
      latest = await browser.execute((expectedPath) => {
        const tab = (
          window as Window & {
            __escodeTabStoreE2E?: {
              getState?: () => {
                tabs?: Array<{
                  label?: string;
                  remoteSessionId?: string;
                  workspaceIdentity?: string;
                  workspacePath?: string;
                }>;
              };
            };
          }
        ).__escodeTabStoreE2E
          ?.getState?.()
          .tabs?.find((candidate) => candidate.workspacePath === expectedPath);
        if (!tab) return null;
        return {
          label: tab.label ?? null,
          remoteSessionId: tab.remoteSessionId ?? null,
          workspaceIdentity: tab.workspaceIdentity ?? null,
          workspacePath: tab.workspacePath ?? null,
        };
      }, workspacePath);
      return latest?.workspaceIdentity === expectedIdentity;
    },
    {
      timeout: 60_000,
      interval: 500,
      timeoutMsg: `${caseMarker}: 重启后没有恢复 ${workspacePath} 的断连远端 tab`,
    },
  );
  if (!latest) throw new Error(`${caseMarker}: 重启后远端 tab 快照缺失`);
  return latest;
}

export async function readRemoteTabs(): Promise<RemoteTabSnapshot[]> {
  return browser.execute(() => {
    const tabs = (
      window as Window & {
        __escodeTabStoreE2E?: {
          getState?: () => {
            tabs?: Array<{
              label?: string;
              remoteSessionId?: string;
              workspaceIdentity?: string;
              workspacePath?: string;
            }>;
          };
        };
      }
    ).__escodeTabStoreE2E?.getState?.().tabs;
    return (tabs ?? []).map((tab) => ({
      label: tab.label ?? null,
      remoteSessionId: tab.remoteSessionId ?? null,
      workspaceIdentity: tab.workspaceIdentity ?? null,
      workspacePath: tab.workspacePath ?? null,
    }));
  });
}

export async function readHostProcessIdentities(): Promise<Array<{ name: string; pid: number }>> {
  await openResourceManager();
  await switchToElectronRendererTarget(isResourceManagerUrl);
  await waitForResourceManagerReady();
  const hosts = await listHostProcessIdentities();
  await switchToElectronRendererTarget(isMainRendererUrl);
  return hosts;
}

/** 等待 Host 把 Source 级 `lastOnlineAt` 写入 settings（S4-e 启动自动连接门禁）。 */
export async function waitForPersistedSourceLastOnlineAt(
  caseMarker: string,
  sourceServerId: string,
): Promise<number> {
  let latest: number | null = null;
  await browser.waitUntil(
    async () => {
      const settings = await readSettings();
      const server = settings.sourceServers?.find(
        (candidate) => candidate.sourceServerId === sourceServerId,
      );
      latest = typeof server?.lastOnlineAt === "number" ? server.lastOnlineAt : null;
      return latest !== null && latest > 0;
    },
    {
      timeout: 30_000,
      interval: 500,
      timeoutMsg: `${caseMarker}: Source ${sourceServerId} 的 lastOnlineAt 没有写入 settings`,
    },
  );
  if (latest === null) throw new Error(`${caseMarker}: settings 缺少 lastOnlineAt`);
  return latest;
}

export async function waitForPersistedActiveRemoteWorkspace(
  caseMarker: string,
  expectedIdentity: string,
): Promise<PersistedRemoteWorkspace> {
  let latest: PersistedRemoteWorkspace | null = null;
  await browser.waitUntil(
    async () => {
      const settings = await readSettings();
      const active = settings.lastActiveWorkspace;
      latest =
        active?.kind === "remote" && active.workspaceIdentity === expectedIdentity
          ? { target: active.target, workspacePath: active.workspacePath }
          : null;
      return Boolean(latest);
    },
    {
      timeout: 30_000,
      interval: 500,
      timeoutMsg: `${caseMarker}: 活动远端 workspace 没有写入 settings`,
    },
  );
  if (!latest) throw new Error(`${caseMarker}: settings 缺少活动远端 workspace`);
  return latest;
}
