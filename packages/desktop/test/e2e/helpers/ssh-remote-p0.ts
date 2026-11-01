/* eslint-disable max-lines -- SSH P0 helper 集中维护真实连接、远端身份、进程与 replay tunnel 生命周期，拆散会隐藏同一条安全边界。 */
import { createConnection, createServer, type Server, type Socket } from "node:net";
import { posix } from "node:path";
import { Client as SSHClient, type ClientChannel, type TcpConnectionDetails } from "ssh2";
import {
  TID_COMPOSER_ADD_DEVICE,
  TID_COMPOSER_WORKSPACE_TRIGGER,
  TID_CONFIRM_DIALOG_CONFIRM,
  TID_DIRECTORY_BROWSER_PATH,
  TID_REMOTE_KIND_SSH,
  TID_SSH_DIALOG,
  TID_SSH_ERROR,
  TID_SSH_HOST_INPUT,
  TID_SSH_PASSWORD_INPUT,
  TID_SSH_PORT_INPUT,
  TID_SSH_SUCCESS,
  TID_SSH_USERNAME_INPUT,
} from "@escode/shared";
import {
  clickTestIdByWebDriver,
  setInputValueByTestIdDom,
  waitForTestIdByDom,
  waitForWorkspaceApp,
} from "./desktop-app.js";
import { resolveSeededReplayBaseUrl } from "./custom-openai-provider-store.js";
import { startNewV4Draft } from "./v4-conversation.js";
import {
  isMainRendererUrl,
  isResourceManagerUrl,
  listHostProcessIdentities,
  openResourceManager,
  switchToElectronRendererTarget,
  waitForResourceManagerReady,
} from "./resource-manager.js";

export const SSH_CONNECT_TIMEOUT_MS = 10 * 60_000;
export const SSH_WORKSPACE_READY_TIMEOUT_MS = 2 * 60_000;

const REMOTE_BOUNDARY_CONFIRM_LABELS: readonly string[] = ["Got it", "知道了"];

/**
 * 首次远程连接成功后会先展示 Source Server 使用边界介绍，只有显式「知道了」才写入
 * `remoteBoundaryIntroAcknowledgedVersion: 1`。SSH E2E 的目标不是介绍页本身；在等待
 * 连接完成的轮询里点掉它，避免把向导停在介绍弹窗误判成连接未完成。
 */
async function acknowledgeRemoteBoundaryIfVisible(): Promise<boolean> {
  return browser.execute(
    (confirmTestId, labels) => {
      const button = document.querySelector<HTMLButtonElement>(`[data-testid="${confirmTestId}"]`);
      if (!button?.getClientRects().length) return false;
      if (!labels.includes(button.innerText.trim())) return false;
      button.click();
      return true;
    },
    TID_CONFIRM_DIALOG_CONFIRM,
    [...REMOTE_BOUNDARY_CONFIRM_LABELS],
  );
}

export interface SSHConnectionConfig {
  host: string;
  password: string;
  port: number;
  username: string;
}

export interface SSHRuntimeConfig extends SSHConnectionConfig {
  workspacePaths: readonly [string, string];
}

export interface SingleSSHWorkspaceRuntimeConfig extends SSHConnectionConfig {
  workspacePath: string;
}

export interface DualSSHRuntimeConfig {
  targets: readonly [SSHConnectionConfig, SSHConnectionConfig];
  workspacePath: string;
}

export interface RemoteTabSnapshot {
  activeWorkspaceIdentity: string | null;
  activeWorkspacePath: string | null;
  remoteHost: string | null;
  remoteKind: string | null;
  remoteSessionId: string | null;
  secretPersistedInTab: boolean;
  workspaceIdentity: string | null;
  workspacePath: string | null;
}

export interface ReplayTunnel {
  baseURL: string;
  close(): Promise<void>;
}

export interface FirstConnectionStallingSSHProxy {
  firstConnectionAccepted: Promise<void>;
  port: number;
  waitForFirstConnectionClosed(): Promise<void>;
  close(): Promise<void>;
}

export async function createFirstConnectionStallingSSHProxy(
  config: Pick<SSHRuntimeConfig, "host" | "port">,
  caseMarker: string,
): Promise<FirstConnectionStallingSSHProxy> {
  let markFirstConnectionAccepted!: () => void;
  const firstConnectionAccepted = new Promise<void>((resolve) => {
    markFirstConnectionAccepted = resolve;
  });
  let markFirstConnectionClosed!: () => void;
  const firstConnectionClosed = new Promise<void>((resolve) => {
    markFirstConnectionClosed = resolve;
  });
  const sockets = new Set<Socket>();
  let connectionCount = 0;
  const trackSocket = (socket: Socket) => {
    sockets.add(socket);
    socket.once("close", () => sockets.delete(socket));
  };
  const server = createServer((client) => {
    trackSocket(client);
    connectionCount += 1;
    if (connectionCount === 1) {
      // 第一个 TCP connection 故意不转发 SSH 握手，稳定保留 pending cancel 窗口。
      markFirstConnectionAccepted();
      client.once("error", () => client.destroy());
      client.once("close", markFirstConnectionClosed);
      // 原因：SSH 客户端一建连就写入 banner，取消时子进程被 SIGKILL，FIN 排在未读 banner 之后。
      // 暂停的 readable 侧读不到 EOF，Node 只停在 CLOSE_WAIT，永远不会触发 end/close，
      // 于是「取消后 socket 未关闭」其实是测试代理没消费收件箱（实测无 resume 时 5s 内只有 accepted）。
      // 修复：resume 丢弃字节让 EOF 正常上浮；不转发、不主动 destroy，关闭仍以真实 FIN 为准。
      client.resume();
      return;
    }

    const upstream = createConnection({ host: config.host, port: config.port });
    trackSocket(upstream);
    const closeBoth = () => {
      client.destroy();
      upstream.destroy();
    };
    client.once("error", closeBoth);
    upstream.once("error", closeBoth);
    upstream.once("connect", () => client.pipe(upstream).pipe(client));
    client.once("close", () => upstream.destroy());
    upstream.once("close", () => client.destroy());
  });
  const port = await listenOnLoopback(server, caseMarker);

  return {
    firstConnectionAccepted,
    port,
    waitForFirstConnectionClosed: () => firstConnectionClosed,
    async close() {
      for (const socket of sockets) {
        socket.destroy();
      }
      await closeServer(server);
    },
  };
}

export function readSSHRuntimeConfig(caseMarker: string): SSHRuntimeConfig {
  const host = requireSecretEnv("ESCODE_E2E_SSH_HOST", caseMarker);
  const username = requireSecretEnv("ESCODE_E2E_SSH_USERNAME", caseMarker);
  const password = requireSecretEnv("ESCODE_E2E_SSH_PASSWORD", caseMarker);
  const portText = process.env.ESCODE_E2E_SSH_PORT?.trim() || "22";
  const port = Number.parseInt(portText, 10);
  if (!Number.isSafeInteger(port) || port < 1 || port > 65_535) {
    throw new Error(`${caseMarker}: ESCODE_E2E_SSH_PORT 必须是 1..65535 的整数`);
  }

  const workspacePaths = (
    process.env.ESCODE_E2E_SSH_WORKSPACE_PATHS?.split(",") ?? ["/root", "/home"]
  )
    .map((value) => value.trim())
    .filter(Boolean);
  if (
    workspacePaths.length !== 2 ||
    workspacePaths.some((value) => !value.startsWith("/")) ||
    workspacePaths[0] === workspacePaths[1]
  ) {
    throw new Error(`${caseMarker}: ESCODE_E2E_SSH_WORKSPACE_PATHS 必须是两个不同的绝对路径`);
  }

  return {
    host,
    password,
    port,
    username,
    workspacePaths: [workspacePaths[0]!, workspacePaths[1]!],
  };
}

export function readSingleSSHWorkspaceRuntimeConfig(
  caseMarker: string,
): SingleSSHWorkspaceRuntimeConfig {
  const host = requireSecretEnv("ESCODE_E2E_SSH_HOST", caseMarker);
  const username = requireSecretEnv("ESCODE_E2E_SSH_USERNAME", caseMarker);
  const password = requireSecretEnv("ESCODE_E2E_SSH_PASSWORD", caseMarker);
  const port = parseSSHPort(
    process.env.ESCODE_E2E_SSH_PORT?.trim() || "22",
    `${caseMarker}: ESCODE_E2E_SSH_PORT`,
  );
  const workspacePath =
    process.env.ESCODE_E2E_SSH_WORKSPACE_PATH?.trim() ||
    process.env.ESCODE_E2E_SSH_SHARED_WORKSPACE_PATH?.trim() ||
    "/root/workspace";
  if (!workspacePath.startsWith("/")) {
    throw new Error(`${caseMarker}: ESCODE_E2E_SSH_WORKSPACE_PATH 必须是绝对路径`);
  }

  return { host, password, port, username, workspacePath };
}

export function readDualSSHRuntimeConfig(caseMarker: string): DualSSHRuntimeConfig {
  const commonHost = process.env.ESCODE_E2E_SSH_HOST?.trim();
  const commonUsername = process.env.ESCODE_E2E_SSH_USERNAME?.trim();
  const commonPassword = process.env.ESCODE_E2E_SSH_PASSWORD?.trim();
  const workspacePath = process.env.ESCODE_E2E_SSH_SHARED_WORKSPACE_PATH?.trim() || "/workspace";
  if (!workspacePath.startsWith("/")) {
    throw new Error(`${caseMarker}: ESCODE_E2E_SSH_SHARED_WORKSPACE_PATH 必须是绝对路径`);
  }

  const readTarget = (suffix: "A" | "B"): SSHConnectionConfig => {
    const host = process.env[`ESCODE_E2E_SSH_${suffix}_HOST`]?.trim() || commonHost;
    const username = process.env[`ESCODE_E2E_SSH_${suffix}_USERNAME`]?.trim() || commonUsername;
    const password = process.env[`ESCODE_E2E_SSH_${suffix}_PASSWORD`]?.trim() || commonPassword;
    const portText = process.env[`ESCODE_E2E_SSH_${suffix}_PORT`]?.trim();
    if (!host || !username || !password || !portText) {
      throw new Error(`${caseMarker}: dual SSH target ${suffix} 缺少 host/username/password/port`);
    }
    const port = parseSSHPort(portText, `${caseMarker}: target ${suffix}`);
    return { host, password, port, username };
  };

  const targets = [readTarget("A"), readTarget("B")] as const;
  if (
    targets[0].host === targets[1].host &&
    targets[0].port === targets[1].port &&
    targets[0].username === targets[1].username
  ) {
    throw new Error(`${caseMarker}: dual SSH target 必须产生两个不同 remote identity`);
  }
  return { targets, workspacePath };
}

export async function connectSSHWorkspace(options: {
  caseMarker: string;
  config: SSHConnectionConfig;
  workspacePath: string;
  /** 新建项目表单使用的名称；缺省按目录名生成，避免同名冲突。 */
  projectName?: string;
}): Promise<RemoteTabSnapshot> {
  await startNewV4Draft();
  await openSSHRemoteDialog(options.caseMarker);
  await fillSSHPasswordSettings(options.config);
  return completeSSHDeviceConnection(options);
}

/**
 * 「添加设备」表单填完后的共用收尾。
 * S5-e 起连接成功即关闭向导，由 composer 项目选择器承接目录；空目录会进入新建项目表单，
 * 首次保存还需确认目录信任。调用方不能在表单之后继续等待向导内的目录步骤。
 */
export async function completeSSHDeviceConnection(options: {
  caseMarker: string;
  workspacePath: string;
  projectName?: string;
}): Promise<RemoteTabSnapshot> {
  const { caseMarker, workspacePath } = options;
  await clickLastEnabledSSHDialogButton(caseMarker);
  await waitForSSHDialogClosed(caseMarker);
  await selectRemoteFolderProjectDirectory(workspacePath, caseMarker);
  await completeRemoteProjectSelectionIfPrompted(workspacePath, caseMarker, options.projectName);
  await waitForWorkspaceApp(workspacePath, SSH_WORKSPACE_READY_TIMEOUT_MS);
  // 直连目标已由 Rust Host 建立/复用 Profile；bind 后 tab 使用 Source Server 稳定身份，
  // 不能在握手前按 legacy `remote:ssh:*` 预生成。
  return waitForRemoteProjectTabByPath({
    caseMarker,
    expectedPath: workspacePath,
    timeoutMs: SSH_WORKSPACE_READY_TIMEOUT_MS,
  });
}

/**
 * 目录选择器确认后的两条合法路径：命中已有项目直接进入，或弹出新建项目表单。
 * 新建项目首次保存必须确认目录信任；已信任目录不会再出现确认框。
 */
async function completeRemoteProjectSelectionIfPrompted(
  workspacePath: string,
  caseMarker: string,
  projectName?: string,
): Promise<void> {
  // 修复原因：S5-e 起目录确认成功后 ChatRemoteFolderSelection 直接切换到 ProjectEditor，
  // 选择器消失只表示阶段推进，不表示流程结束；保存后首次信任目录还会弹出确认框。
  // 旧实现把「选择器不存在」当作已完成，跳过信任确认后用例一直等待向导关闭。
  const mode = await browser.waitUntil(
    async () => {
      const state = await browser.execute(() => ({
        editor: Boolean(
          document
            .querySelector<HTMLElement>('[data-testid="virtual-project-name"]')
            ?.getClientRects().length,
        ),
        picker: Boolean(
          document
            .querySelector<HTMLElement>('[data-testid="chat-remote-folder"]')
            ?.getClientRects().length,
        ),
      }));
      if (state.editor) return "editor" as const;
      if (!state.picker) return "closed" as const;
      return false;
    },
    {
      timeout: 30_000,
      interval: 200,
      timeoutMsg: `${caseMarker}: 目录确认后既没有进入项目，也没有打开新建项目表单`,
    },
  );
  if (mode === "closed") return;

  const name = projectName ?? `E2E ${posix.basename(workspacePath)} ${Date.now().toString(36)}`;
  await setInputValueByTestIdDom("virtual-project-name", name);
  const saveButton = await $('[data-testid="virtual-project-save"]');
  await saveButton.waitForEnabled({
    timeout: 10_000,
    timeoutMsg: `${caseMarker}: 项目名已填写但保存按钮不可用`,
  });
  await saveButton.click();
  // 保存后有两种合法结局：未信任目录先弹确认框，或已信任目录直接创建并关闭表单。
  // 必须等确认框真正出现并点击，不能再以选择器缺失提前返回。
  await browser.waitUntil(
    async () => {
      const state = await browser.execute(() => {
        const visible = (testId: string) =>
          Boolean(
            document.querySelector<HTMLElement>(`[data-testid="${testId}"]`)?.getClientRects()
              .length,
          );
        const trust = document.querySelector<HTMLButtonElement>(
          '[data-testid="project-folder-trust-confirm"]',
        );
        if (trust?.getClientRects().length) {
          trust.click();
          return "trusted" as const;
        }
        // 已信任目录不会出现确认框；只有编辑器与选择器都卸载才算真正完成。
        return visible("virtual-project-name") || visible("chat-remote-folder")
          ? ("pending" as const)
          : ("closed" as const);
      });
      return state !== "pending";
    },
    {
      timeout: 30_000,
      interval: 200,
      timeoutMsg: `${caseMarker}: 保存远端项目后既没有确认目录信任，也没有关闭表单`,
    },
  );
}

export async function openSSHRemoteDialog(caseMarker: string): Promise<void> {
  // 草稿 composer 已切到 ProjectComposerMenu，远程连接入口由 `composer-add-device` 提供；
  // 旧 ChatEmptyState 的 `composer-remote-connection` 不再渲染，不能再作为共享入口。
  await openSSHDevicePicker(caseMarker);
  // 只负责打开到 SSH 设置表单，不能在调用方填表前点主按钮：向导跨次保留草稿
  // （`preserveDraftOnOpen`），提前点击会用上一次的目标直接发起连接并跳到目录步骤。
  await waitForTestIdByDom(TID_SSH_HOST_INPUT, {
    timeout: 30_000,
    timeoutMsg: `${caseMarker}: SSH 向导没有进入设置表单`,
  });
}

/** 从 composer workspace 菜单走「添加设备」入口：连接成功后由项目选择器再选目录。 */
export async function openSSHDevicePicker(caseMarker: string): Promise<void> {
  await clickTestIdByWebDriver(TID_COMPOSER_WORKSPACE_TRIGGER, {
    timeout: 30_000,
    timeoutMsg: `${caseMarker}: 空草稿没有显示 workspace 菜单`,
  });
  try {
    await clickTestIdByWebDriver(TID_COMPOSER_ADD_DEVICE, {
      timeout: 30_000,
      timeoutMsg: `${caseMarker}: workspace 菜单没有显示添加设备入口`,
    });
  } catch (error) {
    const visible = await browser
      .execute(() =>
        Array.from(document.querySelectorAll("[data-testid]"))
          .filter((element) => (element as HTMLElement).getClientRects().length > 0)
          .map((element) => element.getAttribute("data-testid")),
      )
      .catch(() => [] as (string | null)[]);
    throw new Error(
      `${error instanceof Error ? error.message : String(error)} [visible testids: ${visible.join(", ")}]`,
    );
  }
  await clickTestIdByWebDriver(TID_REMOTE_KIND_SSH, {
    timeout: 30_000,
    timeoutMsg: `${caseMarker}: 远程连接向导没有显示 SSH transport`,
  });
}

export async function waitForSSHDialogClosed(caseMarker: string): Promise<void> {
  await browser.waitUntil(
    async () => {
      await acknowledgeRemoteBoundaryIfVisible();
      return browser.execute(
        (dialogTestId) => !document.querySelector(`[data-testid="${dialogTestId}"]`),
        TID_SSH_DIALOG,
      );
    },
    {
      timeout: SSH_CONNECT_TIMEOUT_MS,
      interval: 250,
      timeoutMsg: `${caseMarker}: SSH 连接后向导没有关闭`,
    },
  );
}

/** 在「添加设备」完成后的项目选择器里选定目录（空目录会进入新建项目表单）。 */
export async function selectRemoteFolderProjectDirectory(
  directoryPath: string,
  caseMarker: string,
  expectedEntry?: string,
): Promise<void> {
  try {
    await browser.waitUntil(
      () =>
        browser.execute(() =>
          Boolean(document.querySelector('[data-testid="chat-remote-folder"]')),
        ),
      {
        timeout: SSH_WORKSPACE_READY_TIMEOUT_MS,
        interval: 250,
        timeoutMsg: `${caseMarker}: 连接成功后没有打开远端项目选择器`,
      },
    );
  } catch (error) {
    const snapshot = await browser
      .execute(() => ({
        visibleTestIds: Array.from(document.querySelectorAll("[data-testid]"))
          .filter((element) => (element as HTMLElement).getClientRects().length > 0)
          .map((element) => element.getAttribute("data-testid")),
        bodyText: (document.body.innerText ?? "").slice(0, 1_500),
      }))
      .catch(() => null);
    throw new Error(
      `${error instanceof Error ? error.message : String(error)} [visible testids: ${
        snapshot?.visibleTestIds.join(", ") ?? "unavailable"
      }] [body: ${snapshot?.bodyText.replaceAll("\n", " | ") ?? "unavailable"}]`,
    );
  }
  // 修复原因：目录选择器挂载后先异步加载远端 home；初始导航结束前输入会与 home 结果
  // 竞争，确认按钮还可能在 currentPath 仍是 home 时可用。先等初始输入值就绪再写目标路径。
  try {
    await browser.waitUntil(
      () =>
        browser.execute(() => {
          const input = document.querySelector<HTMLInputElement>(
            '[data-testid="directory-browser-path"]',
          );
          const confirm = document.querySelector<HTMLButtonElement>(
            '[data-testid="directory-browser-confirm"]',
          );
          return Boolean(input?.value) && confirm?.disabled === false;
        }),
      {
        timeout: 30_000,
        interval: 100,
        timeoutMsg: `${caseMarker}: 目录选择器初始目录未就绪`,
      },
    );
  } catch (error) {
    // 初始目录由宿主 readDirectory 决定；失败时把对话框内可见错误一起带出，
    // 否则 CI 只能看到「未就绪」，无法区分 host-offline / 路径归一 / 权限问题。
    const snapshot = await browser
      .execute(() => {
        const dialog = document.querySelector('[data-testid="chat-remote-folder"]');
        const input = document.querySelector<HTMLInputElement>(
          '[data-testid="directory-browser-path"]',
        );
        const confirm = document.querySelector<HTMLButtonElement>(
          '[data-testid="directory-browser-confirm"]',
        );
        return {
          dialogText: (dialog?.textContent ?? "").slice(0, 800),
          inputValue: input?.value ?? null,
          confirmDisabled: confirm?.disabled ?? null,
        };
      })
      .catch(() => null);
    throw new Error(
      `${error instanceof Error ? error.message : String(error)} [dialog: ${
        snapshot?.dialogText.replaceAll("\n", " | ") ?? "unavailable"
      }] [path: ${snapshot?.inputValue ?? "unavailable"}] [confirmDisabled: ${
        snapshot?.confirmDisabled ?? "unavailable"
      }]`,
    );
  }
  await setInputValueByTestIdDom(TID_DIRECTORY_BROWSER_PATH, directoryPath);
  // input/change 是连续事件，React 可能延后提交；提交表单前等两帧，避免提交读到旧 state。
  await browser.execute(
    () =>
      new Promise<void>((resolve) => {
        requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
      }),
  );
  // 修复原因：确认按钮读取的是导航完成后的 currentPath，而不是 input.value。
  // WebDriver 的 Enter 只作用于当前焦点元素，Dialog 的焦点管理会让按键到不了 form，
  // 于是 input 显示目标路径、currentPath 仍停在 home，确认后项目 root 变成 /root。
  // 这里点击 form 的提交按钮，并用目标目录内的标记子目录证明 readdir 已按输入路径完成。
  const submitted = await browser.execute(() => {
    const submit = document.querySelector<HTMLButtonElement>(
      '[data-testid="chat-remote-folder"] form button[type="submit"]',
    );
    if (!submit || submit.disabled) return false;
    submit.click();
    return true;
  });
  if (!submitted) throw new Error(`${caseMarker}: 目录选择器没有可用的路径提交按钮`);
  if (expectedEntry) {
    await browser.waitUntil(
      () =>
        browser.execute((entryName) => {
          const dialog = document.querySelector('[data-testid="chat-remote-folder"]');
          if (!dialog) return false;
          return Array.from(dialog.querySelectorAll("button")).some(
            (button) => (button.textContent ?? "").trim() === entryName,
          );
        }, expectedEntry),
      {
        timeout: 30_000,
        interval: 250,
        timeoutMsg: `${caseMarker}: 目录选择器没有进入目标目录（未看到标记目录 ${expectedEntry}）`,
      },
    );
  }
  try {
    // 修复原因：路径只存在于 input.value，dialog 的 textContent 永远不含它；导航成功的
    // 可观察信号是 readdir 完成后确认按钮解除 disabled（loading/error 期间保持禁用）。
    await browser.waitUntil(
      () =>
        browser.execute((expectedPath) => {
          const input = document.querySelector<HTMLInputElement>(
            '[data-testid="directory-browser-path"]',
          );
          const confirm = document.querySelector<HTMLButtonElement>(
            '[data-testid="directory-browser-confirm"]',
          );
          return input?.value === expectedPath && confirm?.disabled === false;
        }, directoryPath),
      {
        timeout: 30_000,
        interval: 250,
        timeoutMsg: `${caseMarker}: 项目选择器没有进入目标目录`,
      },
    );
  } catch (error) {
    const snapshot = await browser
      .execute(() => ({
        dialogText: (
          document.querySelector('[data-testid="chat-remote-folder"]')?.textContent ?? ""
        ).slice(0, 800),
        inputValue: (
          document.querySelector<HTMLInputElement>('[data-testid="directory-browser-path"]')
            ?.value ?? ""
        ).slice(0, 300),
        confirmDisabled:
          document.querySelector<HTMLButtonElement>('[data-testid="directory-browser-confirm"]')
            ?.disabled ?? null,
      }))
      .catch(() => null);
    throw new Error(
      `${error instanceof Error ? error.message : String(error)} [dialog: ${
        snapshot?.dialogText.replaceAll("\n", " | ") ?? "unavailable"
      }] [input: ${snapshot?.inputValue ?? "unavailable"}] [confirmDisabled: ${
        snapshot?.confirmDisabled ?? "unavailable"
      }]`,
    );
  }
  await clickTestIdByWebDriver("directory-browser-confirm", {
    timeout: SSH_WORKSPACE_READY_TIMEOUT_MS,
    timeoutMsg: `${caseMarker}: 项目选择器没有可用的目录确认按钮`,
  });
}

export async function fillSSHPasswordSettings(config: SSHConnectionConfig): Promise<void> {
  await setInputValueByTestIdDom(TID_SSH_HOST_INPUT, config.host);
  await setInputValueByTestIdDom(TID_SSH_PORT_INPUT, String(config.port));
  await setInputValueByTestIdDom(TID_SSH_USERNAME_INPUT, config.username);
  await setInputValueByTestIdDom(TID_SSH_PASSWORD_INPUT, config.password);
}

export async function clickLastEnabledSSHDialogButton(caseMarker: string): Promise<void> {
  let lastState = "dialog-missing";
  await browser.waitUntil(
    async () => {
      const result = await browser.execute((dialogTestId) => {
        const dialog = document.querySelector<HTMLElement>(`[data-testid="${dialogTestId}"]`);
        if (!dialog || dialog.getClientRects().length === 0) {
          return { clicked: false, reason: "dialog-missing" };
        }
        const button = Array.from(dialog.querySelectorAll<HTMLButtonElement>("button"))
          .filter((candidate) => candidate.getClientRects().length > 0)
          .at(-1);
        if (!button) return { clicked: false, reason: "button-missing" };
        if (button.disabled) {
          return { clicked: false, reason: "button-disabled" };
        }
        button.click();
        return { clicked: true, reason: "clicked" };
      }, TID_SSH_DIALOG);
      lastState = result.reason;
      return result.clicked;
    },
    {
      timeout: 30_000,
      interval: 100,
      timeoutMsg: `${caseMarker}: SSH 向导主动作不可点击 (${lastState})`,
    },
  );
}

/** 失败步骤的次要按钮（Back to configuration）排在最后，重试必须按可见动作名点击。 */
export async function clickSSHDialogRetry(caseMarker: string): Promise<void> {
  let visibleLabels: string[] = [];
  await browser.waitUntil(
    async () => {
      const result = await browser.execute((dialogTestId) => {
        const dialog = document.querySelector<HTMLElement>(`[data-testid="${dialogTestId}"]`);
        if (!dialog?.getClientRects().length) {
          return { clicked: false, labels: [] as string[] };
        }
        const buttons = Array.from(dialog.querySelectorAll<HTMLButtonElement>("button")).filter(
          (button) => button.getClientRects().length > 0 && !button.disabled,
        );
        const labels = buttons.map((button) => button.innerText.trim());
        const retry = buttons.find((button) => ["Retry", "重试"].includes(button.innerText.trim()));
        retry?.click();
        return { clicked: Boolean(retry), labels };
      }, TID_SSH_DIALOG);
      visibleLabels = result.labels;
      return result.clicked;
    },
    {
      timeout: 15_000,
      timeoutMsg: `${caseMarker}: SSH 失败页没有可用的 Retry 按钮 (${visibleLabels.join(",")})`,
    },
  );
}

export async function waitForSSHError(
  caseMarker: string,
  timeout = SSH_CONNECT_TIMEOUT_MS,
): Promise<string> {
  let errorText = "";
  await browser.waitUntil(
    async () => {
      errorText = await browser.execute((errorTestId) => {
        const error = document.querySelector<HTMLElement>(`[data-testid="${errorTestId}"]`);
        return error?.getClientRects().length ? error.innerText.trim() : "";
      }, TID_SSH_ERROR);
      return Boolean(errorText);
    },
    {
      timeout,
      interval: 500,
      timeoutMsg: `${caseMarker}: SSH 失败后没有显示错误`,
    },
  );
  return errorText;
}

export async function waitForSSHDirectoryStep(caseMarker: string): Promise<void> {
  let state: { error: string; status: "connecting" | "error" | "success" } = {
    error: "",
    status: "connecting",
  };
  await browser.waitUntil(
    async () => {
      await acknowledgeRemoteBoundaryIfVisible();
      state = await browser.execute(
        (successTestId, errorTestId) => {
          const success = document.querySelector<HTMLElement>(`[data-testid="${successTestId}"]`);
          if (success?.getClientRects().length) {
            return { error: "", status: "success" as const };
          }
          const error = document.querySelector<HTMLElement>(`[data-testid="${errorTestId}"]`);
          if (error?.getClientRects().length) {
            return { error: error.innerText.trim(), status: "error" as const };
          }
          return { error: "", status: "connecting" as const };
        },
        TID_SSH_SUCCESS,
        TID_SSH_ERROR,
      );
      return state.status !== "connecting";
    },
    {
      timeout: SSH_CONNECT_TIMEOUT_MS,
      interval: 500,
      timeoutMsg: `${caseMarker}: SSH 部署/连接没有完成`,
    },
  );
  if (state.status === "error") {
    throw new Error(`${caseMarker}: SSH 连接失败: ${state.error}`);
  }
}

export interface SelectSSHRemoteDirectoryOptions {
  /**
   * 空目录会进入「新建项目」表单，向导保持打开；开启后等表单出现而不是等向导关闭。
   */
  expectProjectForm?: boolean;
}

/** 新建项目表单的输入框 testid（远端空目录入口）。 */
const TID_VIRTUAL_PROJECT_NAME = "virtual-project-name";

export async function selectSSHRemoteDirectory(
  workspacePath: string,
  caseMarker: string,
  options: SelectSSHRemoteDirectoryOptions = {},
): Promise<void> {
  await browser.waitUntil(
    () =>
      browser.execute((successTestId) => {
        const success = document.querySelector<HTMLElement>(`[data-testid="${successTestId}"]`);
        return Boolean(success?.querySelector<HTMLInputElement>('input[type="text"]'));
      }, TID_SSH_SUCCESS),
    {
      timeout: 30_000,
      timeoutMsg: `${caseMarker}: SSH directory step 没有路径输入框`,
    },
  );

  const normalizedPath = workspacePath.replace(/\/+$/, "") || "/";
  const lastSlash = normalizedPath.lastIndexOf("/");
  const parentPath = lastSlash === 0 ? "/" : normalizedPath.slice(0, lastSlash);
  const directoryName = normalizedPath.slice(lastSlash + 1);
  if (!directoryName) {
    throw new Error(`${caseMarker}: P0 helper 暂不支持选择根目录`);
  }

  const pathInput = $(`[data-testid="${TID_SSH_SUCCESS}"] input[type="text"]`);
  await pathInput.setValue(parentPath);
  await browser.keys("Enter");

  // 先等父目录真实加载出目标项，再点击进入目标目录。仅等待受控 input 的值会把
  // inputPath 更新误判成 currentPath 已切换，导致仍然选择初始 homedir。
  await browser.waitUntil(
    async () =>
      browser.execute(
        (successTestId, expectedName) => {
          const success = document.querySelector<HTMLElement>(`[data-testid="${successTestId}"]`);
          const entry = Array.from(
            success?.querySelectorAll<HTMLButtonElement>("button") ?? [],
          ).find((button) => button.innerText.trim() === expectedName);
          if (!entry) return false;
          entry.click();
          return true;
        },
        TID_SSH_SUCCESS,
        directoryName,
      ),
    {
      timeout: 30_000,
      interval: 100,
      timeoutMsg: `${caseMarker}: 父目录没有显示目标目录 ${directoryName}`,
    },
  );
  await browser.waitUntil(
    () =>
      browser.execute(
        (successTestId, expectedPath) => {
          const input = document
            .querySelector<HTMLElement>(`[data-testid="${successTestId}"]`)
            ?.querySelector<HTMLInputElement>('input[type="text"]');
          return input?.value === expectedPath;
        },
        TID_SSH_SUCCESS,
        normalizedPath,
      ),
    {
      timeout: 30_000,
      interval: 100,
      timeoutMsg: `${caseMarker}: directory browser 没有进入目标路径`,
    },
  );
  await clickLastEnabledSSHDialogButton(caseMarker);
  if (options.expectProjectForm) {
    try {
      await browser.waitUntil(
        () =>
          browser.execute((testId) => {
            const input = document.querySelector<HTMLElement>(`[data-testid="${testId}"]`);
            return Boolean(input && input.getClientRects().length);
          }, TID_VIRTUAL_PROJECT_NAME),
        {
          timeout: SSH_WORKSPACE_READY_TIMEOUT_MS,
          interval: 250,
          timeoutMsg: `${caseMarker}: 选择空目录后没有打开新建项目表单`,
        },
      );
    } catch (error) {
      // 失败时带上向导内的错误文案：产品在选目录后回收 session 时，只有这里能拿到原因。
      const detail = await browser
        .execute((errorTestId) => {
          const element = document.querySelector<HTMLElement>(`[data-testid="${errorTestId}"]`);
          return element?.getClientRects().length ? element.innerText.trim() : "";
        }, TID_SSH_ERROR)
        .catch(() => "");
      throw new Error(
        `${error instanceof Error ? error.message : String(error)}${detail ? `: ${detail}` : ""}`,
      );
    }
    return;
  }
  await browser.waitUntil(
    () =>
      browser.execute(
        (dialogTestId) => !document.querySelector(`[data-testid="${dialogTestId}"]`),
        TID_SSH_DIALOG,
      ),
    {
      timeout: SSH_WORKSPACE_READY_TIMEOUT_MS,
      interval: 250,
      timeoutMsg: `${caseMarker}: 选择远端目录后 SSH 向导没有关闭`,
    },
  );
}

export async function waitForRemoteTab(options: {
  caseMarker: string;
  expectedIdentity: string;
  expectedPath: string;
  expectedRemoteSessionId?: string | null;
  requireActive?: boolean;
  timeoutMs?: number;
}): Promise<RemoteTabSnapshot> {
  let snapshot: RemoteTabSnapshot | null = null;
  try {
    await browser.waitUntil(
      async () => {
        snapshot = await readRemoteTabSnapshot(options.expectedIdentity);
        return (
          snapshot.workspacePath === options.expectedPath &&
          snapshot.workspaceIdentity === options.expectedIdentity &&
          Boolean(snapshot.remoteSessionId) &&
          (!options.expectedRemoteSessionId ||
            snapshot.remoteSessionId === options.expectedRemoteSessionId) &&
          (options.requireActive === false ||
            (snapshot.activeWorkspacePath === options.expectedPath &&
              snapshot.activeWorkspaceIdentity === options.expectedIdentity))
        );
      },
      {
        timeout: options.timeoutMs ?? SSH_WORKSPACE_READY_TIMEOUT_MS,
        interval: 1_000,
        timeoutMsg: `${options.caseMarker}: remote tab identity/session 没有收敛`,
      },
    );
  } catch (error) {
    throw new Error(
      `${options.caseMarker}: remote tab identity/session 没有收敛，latest=${JSON.stringify(snapshot)}`,
      { cause: error },
    );
  }
  if (!snapshot) {
    throw new Error(`${options.caseMarker}: remote tab snapshot 缺失`);
  }
  return snapshot;
}

/** 新建项目用例在握手前拿不到 Source ID，只能按真实 cwd 找唯一远端 tab。 */
export async function waitForRemoteProjectTabByPath(options: {
  caseMarker: string;
  expectedPath: string;
  timeoutMs?: number;
}): Promise<RemoteTabSnapshot> {
  let snapshot: RemoteTabSnapshot | null = null;
  await browser.waitUntil(
    async () => {
      snapshot = await browser.execute((expectedPath) => {
        type RemoteTab = {
          remoteSessionId?: string;
          remoteTarget?: Record<string, unknown>;
          workspaceIdentity?: string;
          workspacePath?: string;
        };
        const store = (
          window as Window & {
            __escodeTabStoreE2E?: {
              getState?: () => {
                activeWorkspaceIdentity?: string | null;
                activeWorkspacePath?: string | null;
                tabs?: RemoteTab[];
              };
            };
          }
        ).__escodeTabStoreE2E;
        const state = store?.getState?.();
        const tab = state?.tabs?.find(
          (candidate) =>
            candidate.workspacePath === expectedPath &&
            candidate.workspaceIdentity?.startsWith("remote:server:"),
        );
        if (!tab) return null;
        const target = tab.remoteTarget;
        return {
          activeWorkspaceIdentity: state?.activeWorkspaceIdentity ?? null,
          activeWorkspacePath: state?.activeWorkspacePath ?? null,
          remoteHost: typeof target?.host === "string" ? target.host : null,
          remoteKind: typeof target?.kind === "string" ? target.kind : null,
          remoteSessionId: tab.remoteSessionId ?? null,
          secretPersistedInTab: Boolean(
            target && ("password" in target || "privateKeyPassphrase" in target),
          ),
          workspaceIdentity: tab.workspaceIdentity ?? null,
          workspacePath: tab.workspacePath ?? null,
        };
      }, options.expectedPath);
      return Boolean(snapshot?.remoteSessionId);
    },
    {
      timeout: options.timeoutMs ?? SSH_WORKSPACE_READY_TIMEOUT_MS,
      interval: 1_000,
      timeoutMsg: `${options.caseMarker}: 新建远端项目后没有出现 remote:server tab (${options.expectedPath})`,
    },
  );
  if (!snapshot) throw new Error(`${options.caseMarker}: remote project tab snapshot 缺失`);
  return snapshot;
}

export function readRemoteTabSnapshot(expectedIdentity: string): Promise<RemoteTabSnapshot> {
  return browser.execute((identity) => {
    type RemoteTab = {
      remoteSessionId?: string;
      remoteTarget?: Record<string, unknown>;
      workspaceIdentity?: string;
      workspacePath?: string;
    };
    const store = (
      window as Window & {
        __escodeTabStoreE2E?: {
          getState?: () => {
            activeWorkspaceIdentity?: string | null;
            activeWorkspacePath?: string | null;
            tabs?: RemoteTab[];
          };
        };
      }
    ).__escodeTabStoreE2E;
    const state = store?.getState?.();
    const tab = state?.tabs?.find((candidate) => candidate.workspaceIdentity === identity);
    const target = tab?.remoteTarget;
    return {
      activeWorkspaceIdentity: state?.activeWorkspaceIdentity ?? null,
      activeWorkspacePath: state?.activeWorkspacePath ?? null,
      remoteHost: typeof target?.host === "string" ? target.host : null,
      remoteKind: typeof target?.kind === "string" ? target.kind : null,
      remoteSessionId: tab?.remoteSessionId ?? null,
      secretPersistedInTab: Boolean(
        target && ("password" in target || "privateKeyPassphrase" in target),
      ),
      workspaceIdentity: tab?.workspaceIdentity ?? null,
      workspacePath: tab?.workspacePath ?? null,
    };
  }, expectedIdentity);
}

export async function readHostProcesses(): Promise<Array<{ name: string; pid: number }>> {
  await openResourceManager();
  await switchToElectronRendererTarget(isResourceManagerUrl);
  await waitForResourceManagerReady();
  const hosts = await listHostProcessIdentities();
  await switchToElectronRendererTarget(isMainRendererUrl);
  return hosts;
}

export async function runSSHCommand(
  config: SSHConnectionConfig,
  command: string,
): Promise<{ code: number | null; stderr: string; stdout: string }> {
  const client = await connectSSHClient(config);
  try {
    return await new Promise((resolve, reject) => {
      client.exec(command, (error, channel) => {
        if (error) {
          reject(error);
          return;
        }
        collectSSHChannel(channel).then(resolve, reject);
      });
    });
  } finally {
    client.end();
  }
}

export async function createSSHReplayTunnel(
  config: SSHConnectionConfig,
  caseMarker: string,
): Promise<ReplayTunnel> {
  const replayBaseURL = await resolveSeededReplayBaseUrl();
  if (!replayBaseURL) {
    throw new Error(`${caseMarker}: 没有找到本轮 replay provider URL`);
  }
  const replayURL = new URL(replayBaseURL);
  const replayPort = Number.parseInt(replayURL.port, 10);
  if (!Number.isSafeInteger(replayPort) || replayPort < 1) {
    throw new Error(`${caseMarker}: replay provider URL 缺少有效端口`);
  }

  const client = new SSHClient();
  client.on("tcp connection", (_details: TcpConnectionDetails, accept) => {
    const channel = accept();
    const socket = createConnection({
      host: replayURL.hostname,
      port: replayPort,
    });
    const closeBoth = () => {
      channel.destroy();
      socket.destroy();
    };
    channel.once("error", closeBoth);
    socket.once("error", closeBoth);
    socket.once("connect", () => channel.pipe(socket).pipe(channel));
    socket.once("close", () => channel.destroy());
    channel.once("close", () => socket.destroy());
  });

  const remotePort = await new Promise<number>((resolve, reject) => {
    client.once("error", reject);
    client.once("ready", () => {
      client.forwardIn("127.0.0.1", 0, (error, assignedPort) => {
        if (error) reject(error);
        else resolve(assignedPort);
      });
    });
    client.connect(toSSHConnectOptions(config));
  });

  return {
    baseURL: `http://127.0.0.1:${remotePort}`,
    close: () =>
      new Promise<void>((resolve) => {
        client.unforwardIn("127.0.0.1", remotePort, () => {
          client.end();
          resolve();
        });
      }),
  };
}

function listenOnLoopback(server: Server, caseMarker: string): Promise<number> {
  return new Promise((resolve, reject) => {
    const onError = (error: Error) => reject(error);
    server.once("error", onError);
    server.listen({ host: "127.0.0.1", port: 0 }, () => {
      server.off("error", onError);
      const address = server.address();
      if (!address || typeof address === "string") {
        reject(new Error(`${caseMarker}: 无法解析 SSH pending proxy 监听端口`));
        return;
      }
      resolve(address.port);
    });
  });
}

function closeServer(server: Server): Promise<void> {
  if (!server.listening) {
    return Promise.resolve();
  }
  return new Promise((resolve, reject) => {
    server.close((error) => {
      if (error) reject(error);
      else resolve();
    });
  });
}

function requireSecretEnv(name: string, caseMarker: string): string {
  const value = process.env[name]?.trim();
  if (!value) throw new Error(`${caseMarker}: 缺少 ${name}`);
  return value;
}

function parseSSHPort(portText: string, context: string): number {
  const port = Number.parseInt(portText, 10);
  if (!Number.isSafeInteger(port) || port < 1 || port > 65_535) {
    throw new Error(`${context}: SSH port 必须是 1..65535 的整数`);
  }
  return port;
}

function connectSSHClient(config: SSHConnectionConfig): Promise<SSHClient> {
  return new Promise((resolve, reject) => {
    const client = new SSHClient();
    client.once("error", reject);
    client.once("ready", () => resolve(client));
    client.connect(toSSHConnectOptions(config));
  });
}

function toSSHConnectOptions(config: SSHConnectionConfig) {
  return {
    host: config.host,
    password: config.password,
    port: config.port,
    readyTimeout: SSH_CONNECT_TIMEOUT_MS,
    username: config.username,
  };
}

function collectSSHChannel(
  channel: ClientChannel,
): Promise<{ code: number | null; stderr: string; stdout: string }> {
  return new Promise((resolve, reject) => {
    let stdout = "";
    let stderr = "";
    let exitCode: number | null = null;
    channel.setEncoding("utf8");
    channel.stderr.setEncoding("utf8");
    channel.on("data", (chunk: string) => {
      stdout += chunk;
    });
    channel.stderr.on("data", (chunk: string) => {
      stderr += chunk;
    });
    channel.once("exit", (code: number | null) => {
      exitCode = code;
    });
    channel.once("error", reject);
    channel.once("close", () => resolve({ code: exitCode, stderr, stdout }));
  });
}
