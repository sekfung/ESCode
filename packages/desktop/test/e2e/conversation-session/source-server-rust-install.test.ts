import { randomUUID } from "node:crypto";
import { Client as SSHClient } from "ssh2";
import { buildServerRemoteWorkspaceIdentity, getSourceServerUpdateTarget } from "@escode/shared";
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
import {
  readSingleSSHWorkspaceRuntimeConfig,
  waitForSSHDirectoryStep,
} from "../helpers/ssh-remote-p0.js";
import { readOrSeedRemoteSourceServerId } from "../helpers/source-server-remote-identity.js";

const CASE_MARKER = "SOURCE_SERVER_RUST_INSTALL";
const WORKSPACE_READY_TIMEOUT_MS = 5 * 60_000;
const INSTALL_READY_TIMEOUT_MS = 10 * 60_000;

interface SourceServerState {
  profileId: string;
  state: string;
  currentVersion?: string;
  installPreview?: { sizeBytes?: number; version?: string };
}

type RemoteUpdateResult =
  | { status: "pending" }
  | { status: "ok"; state: SourceServerState }
  | { status: "error"; reason: string; code?: unknown };

describe(`${CASE_MARKER}: Rust Host 首次安装`, () => {
  after(async () => {
    await browser.electron.restoreAllMocks();
    await clearAppData();
  });

  it("空目标首次连接安装 release 后进入 online", async function () {
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

    await waitForDefaultWorkspaceReady();
    await saveCredential(CASE_MARKER, credentialKey, config.password);
    await reloadElectronSessionSafely(browser, {
      afterElectronProcessExit: async () => {
        await seedSettings({
          sourceServers: [
            {
              sourceServerId,
              displayName: "Rust Install E2E",
              preferredProfileId: profileId,
              installState: "installed",
              declaredWorkspaces: [],
            },
          ],
          connectionProfiles: [
            {
              profileId,
              sourceServerId,
              displayName: "Rust Install E2E SSH",
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
    await startRecordingStates();

    await openRemoteProfile(CASE_MARKER, profileId, sourceServerId);
    await waitForSSHDirectoryStep(CASE_MARKER);
    await selectRemoteDirectoryByPath(CASE_MARKER, config.workspacePath);
    await waitForWorkspaceApp(config.workspacePath, WORKSPACE_READY_TIMEOUT_MS);
    await waitForProfileState(CASE_MARKER, profileId, "online", INSTALL_READY_TIMEOUT_MS);

    const session = await waitForRemoteSession(
      CASE_MARKER,
      config.workspacePath,
      workspaceIdentity,
    );
    expect(session.remoteSessionId).toBeTruthy();
    const states = await readRecordedStates();
    const install = states.find((state) => state.state === "needs-install");
    if (!install) throw new Error(`${CASE_MARKER}: states=${JSON.stringify(states)}`);
    expect(install?.installPreview?.sizeBytes ?? 0).toBeGreaterThan(0);
    expect(install?.installPreview?.version).toBe(
      process.env.ESCODE_E2E_SSH_APP_VERSION ?? "3.15.2",
    );

    const logs = await readOperationLogs(profileId);
    expect(logs.some((entry) => entry.message.includes("installed source server"))).toBe(true);
    const remote = await runRemoteCommand(
      config,
      "test -x ~/.escode/server/node && test -f ~/.escode/server/zcode-server.cjs && echo installed",
    );
    expect(remote).toContain("installed");

    const expectedTarget = getSourceServerUpdateTarget({
      kind: "ssh",
      host: config.host,
      port: config.port,
      username: config.username,
    });
    const updated = await runRemoteUpdate(profileId, expectedTarget);
    expect(updated.currentVersion).toBe(process.env.ESCODE_E2E_SSH_APP_VERSION ?? "3.15.2");
    await waitForProfileState(CASE_MARKER, profileId, "online", INSTALL_READY_TIMEOUT_MS);
    const updatedStates = await readRecordedStates();
    expect(updatedStates.some((state) => state.state === "updating")).toBe(true);
    const updatedLogs = await readOperationLogs(profileId);
    expect(updatedLogs.some((entry) => entry.message.startsWith("source server updated to"))).toBe(
      true,
    );

    const hostIdentities = await readHostProcessIdentities();
    expect(hostIdentities.filter((host) => host.name.startsWith("escode-host-remote-"))).toEqual(
      [],
    );
  });
});

async function startRecordingStates(): Promise<void> {
  const recorded = await browser.execute(() => {
    type Scope = Window & {
      __remoteWorkspaceSessionStoreE2E?: {
        getState?: () => {
          baseServices?: {
            sourceServerManagementService?: {
              onDidChangeState: (listener: (delta: { state: SourceServerState }) => void) => {
                dispose(): void;
              };
            } | null;
          };
        };
      };
      __sourceServerInstallStatesE2E?: SourceServerState[];
      __sourceServerInstallStatesDisposeE2E?: { dispose(): void };
    };
    const scope = window as Scope;
    const services =
      scope.__remoteWorkspaceSessionStoreE2E?.getState?.().baseServices
        ?.sourceServerManagementService;
    if (!services) return false;
    scope.__sourceServerInstallStatesDisposeE2E?.dispose();
    scope.__sourceServerInstallStatesE2E = [];
    scope.__sourceServerInstallStatesDisposeE2E = services.onDidChangeState((delta) => {
      scope.__sourceServerInstallStatesE2E?.push(delta.state);
    });
    return true;
  });
  if (!recorded) throw new Error(`${CASE_MARKER}: source-server-management service unavailable`);
}

async function readRecordedStates(): Promise<SourceServerState[]> {
  return browser.execute(
    () =>
      (
        window as Window & {
          __sourceServerInstallStatesE2E?: SourceServerState[];
        }
      ).__sourceServerInstallStatesE2E ?? [],
  );
}

async function runRemoteUpdate(
  profileId: string,
  expectedTarget: unknown,
): Promise<SourceServerState> {
  // 显式更新包含远端安装与新旧连接切换，耗时远超 WebDriver 单次请求默认 30 秒。
  // 这里只负责发起调用，再轮询 Renderer 上保存的最终结果，避免把长操作塞进一次
  // executeAsync 而被协议层超时中断。
  const started = await browser.execute(
    (id, target) => {
      type Service = {
        updateSource: (input: {
          profileId: string;
          force: boolean;
          expectedTarget: unknown;
        }) => Promise<{ states: SourceServerState[] }>;
      };
      type Scope = Window & {
        __remoteWorkspaceSessionStoreE2E?: {
          getState?: () => {
            baseServices?: { sourceServerManagementService?: Service | null } | null;
          };
        };
        __sourceServerUpdateResultE2E?: RemoteUpdateResult;
      };
      const scope = window as Scope;
      const service =
        scope.__remoteWorkspaceSessionStoreE2E?.getState?.().baseServices
          ?.sourceServerManagementService;
      scope.__sourceServerUpdateResultE2E = { status: "pending" };
      if (!service) {
        scope.__sourceServerUpdateResultE2E = {
          status: "error",
          reason: "source-server-management service unavailable",
        };
        return false;
      }
      void service.updateSource({ profileId: id, force: true, expectedTarget: target }).then(
        (snapshot) => {
          const state = snapshot.states.find((candidate) => candidate.profileId === id);
          scope.__sourceServerUpdateResultE2E = state
            ? { status: "ok", state }
            : { status: "error", reason: "profile state missing" };
        },
        (error: unknown) => {
          scope.__sourceServerUpdateResultE2E = {
            status: "error",
            reason: error instanceof Error ? error.message : String(error),
            code:
              error && typeof error === "object" && "code" in error
                ? (error as { code?: unknown }).code
                : undefined,
          };
        },
      );
      return true;
    },
    profileId,
    expectedTarget,
  );
  if (!started) {
    throw new Error(`${CASE_MARKER}: source-server-management service unavailable`);
  }
  let result: RemoteUpdateResult | undefined;
  await browser.waitUntil(
    async () => {
      result = (await browser.execute(
        () =>
          (window as Window & { __sourceServerUpdateResultE2E?: RemoteUpdateResult })
            .__sourceServerUpdateResultE2E,
      )) as RemoteUpdateResult | undefined;
      return result?.status === "ok" || result?.status === "error";
    },
    {
      timeout: INSTALL_READY_TIMEOUT_MS,
      interval: 1_000,
      timeoutMsg: `${CASE_MARKER}: updateSource did not settle`,
    },
  );
  if (result?.status !== "ok" || !result.state) {
    throw new Error(`${CASE_MARKER}: updateSource failed ${JSON.stringify(result)}`);
  }
  return result.state;
}

async function readOperationLogs(profileId: string): Promise<Array<{ message: string }>> {
  const result = await browser.executeAsync((id, done) => {
    const services = (
      window as Window & {
        __remoteWorkspaceSessionStoreE2E?: {
          getState?: () => {
            baseServices?: {
              sourceServerManagementService?: {
                getRecentOperationLogs: (profile: string) => Promise<Array<{ message: string }>>;
              } | null;
            };
          };
        };
      }
    ).__remoteWorkspaceSessionStoreE2E?.getState?.().baseServices?.sourceServerManagementService;
    if (!services) {
      done([]);
      return;
    }
    void services.getRecentOperationLogs(id).then(
      (logs) => done(logs),
      () => done([]),
    );
  }, profileId);
  return (result as Array<{ message: string }> | null) ?? [];
}

async function runRemoteCommand(
  config: { host: string; port: number; username: string; password: string },
  command: string,
): Promise<string> {
  return new Promise((resolve, reject) => {
    const client = new SSHClient();
    client.once("ready", () => {
      client.exec(command, (error, stream) => {
        if (error) {
          client.end();
          reject(error);
          return;
        }
        let stdout = "";
        let stderr = "";
        stream.on("data", (chunk: Buffer) => {
          stdout += chunk.toString("utf8");
        });
        stream.stderr.on("data", (chunk: Buffer) => {
          stderr += chunk.toString("utf8");
        });
        stream.once("close", (code: number | null) => {
          client.end();
          if (code === 0) resolve(stdout.trim());
          else reject(new Error(`${CASE_MARKER}: remote command failed code=${code} ${stderr}`));
        });
      });
    });
    client.once("error", (error) => reject(error));
    client.connect({
      host: config.host,
      port: config.port,
      username: config.username,
      password: config.password,
      readyTimeout: 30_000,
    });
  });
}
