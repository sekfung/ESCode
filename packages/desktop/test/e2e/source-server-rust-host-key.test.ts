import { createHmac, randomUUID } from "node:crypto";
import { existsSync } from "node:fs";
import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import {
  clearAppData,
  getE2EAppDataPaths,
  seedSettings,
  waitForDefaultWorkspaceReady,
} from "./helpers/desktop-app.js";
import { reloadElectronSessionSafely } from "./helpers/e2e-electron-reload.js";
import { saveCredential, waitForProfileState } from "./helpers/source-server-rust-remote.js";
import {
  readSingleSSHWorkspaceRuntimeConfig,
  type SSHConnectionConfig,
} from "./helpers/ssh-remote-p0.js";
import { readOrSeedRemoteSourceServerId } from "./helpers/source-server-remote-identity.js";

const CASE_MARKER = "SOURCE_SERVER_RUST_HOST_KEY";
const CONNECT_TIMEOUT_MS = 10 * 60_000;
// 与真实远端 key 不同的合法 ed25519 公钥；只用于制造 known_hosts 冲突。
const WRONG_ED25519_KEY = "AAAAC3NzaC1lZDI1NTE5AAAAIMC2fn1T0bAOn0YezMpkvC0NPOqXXog08wfLoDQXTbF9";

interface ProfileConnectionState {
  profileId: string;
  state: string;
  error?: string;
  errorCode?: string;
  retryable?: boolean;
}

describe(`${CASE_MARKER}: Rust Host SSH host key 策略`, () => {
  after(async () => {
    await browser.electron.restoreAllMocks();
    await clearAppData();
  });

  it("首次连接写入 Desktop 自有 known_hosts，key 变化时显式失败且不覆盖", async function () {
    if (!process.env.ESCODE_E2E_SSH_HOST?.trim()) {
      this.skip();
    }
    this.timeout(25 * 60_000);
    const config = readSingleSSHWorkspaceRuntimeConfig(CASE_MARKER);
    const sourceServerId = await readOrSeedRemoteSourceServerId(CASE_MARKER, config);
    const profileId = randomUUID();
    const credentialKey = `e2e:ssh:${randomUUID()}`;
    const knownHostsPath = join(getE2EAppDataPaths().homeDir, ".escode", "v2", "known_hosts");

    await waitForDefaultWorkspaceReady();
    await saveCredential(CASE_MARKER, credentialKey, config.password);
    await reloadElectronSessionSafely(browser, {
      afterElectronProcessExit: async () => {
        await seedSettings({
          sourceServers: [
            {
              sourceServerId,
              displayName: "Rust Host Key E2E",
              preferredProfileId: profileId,
              installState: "installed",
              declaredWorkspaces: [],
            },
          ],
          connectionProfiles: [
            {
              profileId,
              sourceServerId,
              displayName: "Rust Host Key E2E SSH",
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
    await installStateRecorder(CASE_MARKER);

    await requestConnect(CASE_MARKER, profileId);
    await waitForProfileState(CASE_MARKER, profileId, "online", CONNECT_TIMEOUT_MS);
    await waitForFile(knownHostsPath);
    const original = await readFile(knownHostsPath, "utf8");
    const hostField = knownHostsField(config);
    expect(original.split("\n").some((line) => matchesKnownHostLine(line, hostField))).toBe(true);

    await disconnectSource(CASE_MARKER, sourceServerId);
    await waitForProfileState(CASE_MARKER, profileId, "absent");

    const tampered = replaceKnownHostKey(original, hostField);
    await writeFile(knownHostsPath, tampered, "utf8");

    await requestConnect(CASE_MARKER, profileId);
    await waitForProfileState(CASE_MARKER, profileId, "failed", CONNECT_TIMEOUT_MS);
    const state = await readRecordedState(profileId);
    expect(state?.errorCode).toBe("remote-host-key-changed");
    expect(state?.retryable).toBe(false);
    expect(state?.error ?? "").toMatch(/REMOTE HOST IDENTIFICATION HAS CHANGED/i);
    // 安全事件不能自动覆盖：文件必须保持被篡改后的内容。
    expect(await readFile(knownHostsPath, "utf8")).toBe(tampered);
  });
});

function knownHostsField(config: SSHConnectionConfig): string {
  return config.port === 22 ? config.host : `[${config.host}]:${config.port}`;
}

function replaceKnownHostKey(contents: string, hostField: string): string {
  const lines = contents.split("\n");
  const index = lines.findIndex((line) => matchesKnownHostLine(line, hostField));
  if (index < 0) {
    throw new Error(`${CASE_MARKER}: known_hosts 没有 ${hostField} 条目`);
  }
  lines[index] = `${hostField} ssh-ed25519 ${WRONG_ED25519_KEY} e2e-wrong-host-key`;
  return lines.join("\n");
}

/** 系统 ssh_config 可能启用 HashKnownHosts：同时接受明文与 `|1|salt|hash` 形式。 */
function matchesKnownHostLine(line: string, hostField: string): boolean {
  const host = line.split(" ")[0];
  if (!host) return false;
  if (host === hostField) return true;
  const parts = host.split("|");
  if (parts.length !== 4 || parts[1] !== "1") return false;
  const salt = Buffer.from(parts[2] ?? "", "base64");
  return createHmac("sha1", salt).update(hostField).digest("base64") === parts[3];
}

async function waitForFile(path: string): Promise<void> {
  await browser.waitUntil(() => existsSync(path), {
    timeout: 30_000,
    interval: 250,
    timeoutMsg: `${CASE_MARKER}: 首次连接没有写入 ${path}`,
  });
}

async function requestConnect(caseMarker: string, profileId: string): Promise<void> {
  const started = await browser.execute((id) => {
    type Service = { connectProfile: (profileId: string) => Promise<unknown> };
    const service = (
      window as Window & {
        __remoteWorkspaceSessionStoreE2E?: {
          getState?: () => {
            baseServices?: {
              sourceServerManagementService?: Service | null;
            } | null;
          };
        };
      }
    ).__remoteWorkspaceSessionStoreE2E?.getState?.().baseServices?.sourceServerManagementService;
    if (!service) return false;
    // 连接包含安装与握手，可能超过单次 WebDriver 请求；只发起并轮询状态。
    void service.connectProfile(id).catch(() => {});
    return true;
  }, profileId);
  if (!started) {
    throw new Error(`${caseMarker}: source-server-management service unavailable`);
  }
}

async function disconnectSource(caseMarker: string, sourceServerId: string): Promise<void> {
  const result = await browser.executeAsync((id, done) => {
    type Service = {
      disconnectSource: (sourceServerId: string) => Promise<unknown>;
    };
    const service = (
      window as Window & {
        __remoteWorkspaceSessionStoreE2E?: {
          getState?: () => {
            baseServices?: {
              sourceServerManagementService?: Service | null;
            } | null;
          };
        };
      }
    ).__remoteWorkspaceSessionStoreE2E?.getState?.().baseServices?.sourceServerManagementService;
    if (!service) {
      done({ ok: false, reason: "service unavailable" });
      return;
    }
    void service.disconnectSource(id).then(
      () => done({ ok: true }),
      (error: unknown) => done({ ok: false, reason: String(error) }),
    );
  }, sourceServerId);
  if (!(result as { ok?: boolean } | null)?.ok) {
    throw new Error(`${caseMarker}: 断开连接失败 ${JSON.stringify(result)}`);
  }
}

async function installStateRecorder(caseMarker: string): Promise<void> {
  // connectProfile 的 rejection 会被 WDIO 当成页面未处理错误；状态事件是唯一
  // 不受该错误干扰的读取路径。
  const installed = await browser.execute(() => {
    type Service = {
      onDidChangeState: (listener: (delta: { state: ProfileConnectionState }) => void) => {
        dispose(): void;
      };
    };
    const scope = window as Window & {
      __sourceServerHostKeyStatesE2E?: ProfileConnectionState[];
      __sourceServerHostKeyDisposeE2E?: { dispose(): void };
      __remoteWorkspaceSessionStoreE2E?: {
        getState?: () => {
          baseServices?: {
            sourceServerManagementService?: Service | null;
          } | null;
        };
      };
    };
    const service =
      scope.__remoteWorkspaceSessionStoreE2E?.getState?.().baseServices
        ?.sourceServerManagementService;
    if (!service) return false;
    scope.__sourceServerHostKeyDisposeE2E?.dispose();
    scope.__sourceServerHostKeyStatesE2E = [];
    scope.__sourceServerHostKeyDisposeE2E = service.onDidChangeState((delta) => {
      scope.__sourceServerHostKeyStatesE2E?.push(delta.state);
    });
    return true;
  });
  if (!installed) {
    throw new Error(`${caseMarker}: source-server-management service unavailable`);
  }
}

async function readRecordedState(profileId: string): Promise<ProfileConnectionState | null> {
  // 失败的 connectProfile 会让 Renderer 的 execute 命令带上该 RPC 错误；
  // 状态只用于断言，改从 Main 读取同一份事件快照，避免错误串到测试命令上。
  const raw = await browser.electron.execute(async (electron, id) => {
    const windows = electron.BrowserWindow.getAllWindows();
    const target = windows.find((window) => window.isVisible()) ?? windows[0];
    if (!target) throw new Error("no Electron window for state read");
    return target.webContents.executeJavaScript(
      `JSON.stringify((window.__sourceServerHostKeyStatesE2E ?? [])
        .filter((state) => state.profileId === ${JSON.stringify(id)})
        .at(-1) ?? null)`,
    );
  }, profileId);
  if (typeof raw !== "string" || raw === "null") return null;
  return JSON.parse(raw) as ProfileConnectionState;
}
