import { createConnection } from "node:net";
import { Client as SSHClient, type TcpConnectionDetails } from "ssh2";
import {
  TID_TASK_ITEM,
  TID_WORKSPACE_ITEM,
  TID_COMPOSER_WORKSPACE_TRIGGER,
  testId,
} from "@escode/shared";
import { clearAppData } from "../helpers/desktop-app.js";
import {
  refreshSeededOpenAIProvidersThroughSettings,
  seedCustomOpenAIChatCompletionsProvider,
} from "../helpers/custom-openai-provider.js";
import { resolveSeededReplayBaseUrl } from "../helpers/custom-openai-provider-store.js";
import { selectUpstreamProviderModelById } from "../helpers/upstream-provider.js";
import {
  getUpstreamRequestRecordCount,
  waitForUpstreamRequest,
} from "../helpers/conversation-session-network.js";
import {
  prepareV4ConversationE2E,
  selectV4TaskById,
  sendV4PromptAndWaitAccepted,
  startNewV4Draft,
  waitForV4AssistantMessageContaining,
  waitForV4ConversationState,
} from "../helpers/v4-conversation.js";
import {
  isMainRendererUrl,
  isResourceManagerUrl,
  openResourceManager,
  switchToElectronRendererTarget,
  waitForResourceManagerReady,
  listHostProcessIdentities,
} from "../helpers/resource-manager.js";
import {
  completeSSHDeviceConnection,
  fillSSHPasswordSettings,
  openSSHRemoteDialog,
  runSSHCommand,
} from "../helpers/ssh-remote-p0.js";

const CASE_MARKER = "R1_SSH_P0_01";
const PROMPT_MARKER = "E2E_SSH_REMOTE_P0";
const MODEL_ID = "e2e-ssh-remote-p0-model";
const CONNECT_TIMEOUT_MS = 10 * 60_000;
const WORKSPACE_READY_TIMEOUT_MS = 2 * 60_000;
const TURN_TIMEOUT_MS = 2 * 60_000;

interface SSHRuntimeConfig {
  host: string;
  password: string;
  port: number;
  username: string;
  workspacePaths: readonly [string, string];
}

interface RemoteTabSnapshot {
  activeWorkspaceIdentity: string | null;
  activeWorkspacePath: string | null;
  remoteHost: string | null;
  remoteKind: string | null;
  remoteSessionId: string | null;
  secretPersistedInTab: boolean;
  workspaceIdentity: string | null;
  workspacePath: string | null;
}

interface ReplayTunnel {
  baseURL: string;
  close(): Promise<void>;
}

interface TaskCase {
  firstPrompt: string;
  firstReply: string;
  followPrompt: string;
  followReply: string;
  identity: string;
  label: "A" | "B";
  remoteSessionId: string;
  taskId: string;
  workspacePath: string;
}

describe(`${CASE_MARKER}: SSH remote workspace P0`, () => {
  let replayTunnel: ReplayTunnel | null = null;

  after(async () => {
    await replayTunnel?.close();
    replayTunnel = null;
    await browser.electron.restoreAllMocks();
    await clearAppData();
  });

  it("R1-SSH-P0-01 在两个 SSH workspace 各自首发，并切回 task 续发", async function () {
    this.timeout(20 * 60_000);
    const config = readSSHRuntimeConfig();
    const [workspaceA, workspaceB] = config.workspacePaths;

    await prepareV4ConversationE2E({ skipProvider: true });
    const initialHosts = await readHostProcesses();
    expect(initialHosts).toHaveLength(1);
    replayTunnel = await createReplayTunnel(config);
    const previousReplayBaseURL = process.env.E2E_PROVIDER_RUNTIME_BASE_URL;
    let provider: { id: string; name: string };
    process.env.E2E_PROVIDER_RUNTIME_BASE_URL = replayTunnel.baseURL;
    try {
      provider = await seedCustomOpenAIChatCompletionsProvider({
        modelId: MODEL_ID,
        providerId: "e2e-ssh-remote-p0-provider",
        providerName: "SSH Remote P0",
      });
      await refreshSeededOpenAIProvidersThroughSettings([provider.name]);
    } finally {
      if (previousReplayBaseURL === undefined) {
        delete process.env.E2E_PROVIDER_RUNTIME_BASE_URL;
      } else {
        process.env.E2E_PROVIDER_RUNTIME_BASE_URL = previousReplayBaseURL;
      }
    }

    const provisioningRecordsBeforeConnect = await readProvisioningRecordCount(config);

    const taskA = await connectWorkspaceAndSendFirstTurn({
      config,
      label: "A",
      provider,
      workspacePath: workspaceA,
    });
    const provisioningRecordsAfterFirstWorkspace = await waitForProvisioningRecordCountGreaterThan(
      config,
      provisioningRecordsBeforeConnect,
    );
    const taskB = await connectWorkspaceAndSendFirstTurn({
      config,
      label: "B",
      provider,
      workspacePath: workspaceB,
    });

    // 修复原因：Provisioning 的同步身份是 Remote Environment，不是 Workspace。
    // 同一 SSH Environment 打开第二个目录时必须复用已经完成的同步，不能再写一条记录。
    expect(await readProvisioningRecordCount(config)).toBe(provisioningRecordsAfterFirstWorkspace);

    expect(taskA.remoteSessionId).not.toBe(taskB.remoteSessionId);
    // S5：远程 scope 由 App 级 Rust Host 服务，不得再出现 per-window Node 远程进程。
    const hostsAfterTask = await readHostProcesses();
    for (const host of initialHosts) {
      expect(hostsAfterTask).toEqual(expect.arrayContaining([host]));
    }
    expect(
      hostsAfterTask.filter((host) => host.name.startsWith("escode-host-remote-")).length,
    ).toBe(0);

    await switchTaskAndSendFollowTurn(taskA, taskB.firstPrompt);
    await switchTaskAndSendFollowTurn(taskB, taskA.firstPrompt);

    await assertTaskRow(taskA, { active: false, running: false });
    await assertTaskRow(taskB, { active: true, running: false });
    // S5：远程 scope 由 App 级 Rust Host 服务，不得再出现 per-window Node 远程进程。
    const hostsAfterFollowTurns = await readHostProcesses();
    for (const host of initialHosts) {
      expect(hostsAfterFollowTurns).toEqual(expect.arrayContaining([host]));
    }
    expect(
      hostsAfterFollowTurns.filter((host) => host.name.startsWith("escode-host-remote-")).length,
    ).toBe(0);
  });
});

async function readProvisioningRecordCount(config: SSHRuntimeConfig): Promise<number> {
  const result = await runSSHCommand(
    config,
    'if [ -f "$HOME/.escode/v2/runtime/provider/provisioning.json" ]; then cat "$HOME/.escode/v2/runtime/provider/provisioning.json"; else printf \'{"records":[]}\'; fi',
  );
  if (result.code !== 0) {
    throw new Error(`${CASE_MARKER}: 读取远端 Provisioning 状态失败：${result.stderr}`);
  }
  const parsed = JSON.parse(result.stdout) as { records?: unknown };
  if (!Array.isArray(parsed.records)) {
    throw new Error(`${CASE_MARKER}: 远端 Provisioning 状态缺少 records`);
  }
  return parsed.records.length;
}

async function waitForProvisioningRecordCountGreaterThan(
  config: SSHRuntimeConfig,
  baseline: number,
): Promise<number> {
  let observed = baseline;
  await browser.waitUntil(
    async () => {
      observed = await readProvisioningRecordCount(config);
      return observed > baseline;
    },
    {
      timeout: 30_000,
      interval: 500,
      timeoutMsg: `${CASE_MARKER}: 首个远端 Workspace 没有完成 Provisioning`,
    },
  );
  return observed;
}

function readSSHRuntimeConfig(): SSHRuntimeConfig {
  const host = requireSecretEnv("ESCODE_E2E_SSH_HOST");
  const username = requireSecretEnv("ESCODE_E2E_SSH_USERNAME");
  const password = requireSecretEnv("ESCODE_E2E_SSH_PASSWORD");
  const portText = process.env.ESCODE_E2E_SSH_PORT?.trim() || "22";
  const port = Number.parseInt(portText, 10);
  if (!Number.isSafeInteger(port) || port < 1 || port > 65_535) {
    throw new Error(`${CASE_MARKER}: ESCODE_E2E_SSH_PORT 必须是 1..65535 的整数`);
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
    throw new Error(`${CASE_MARKER}: ESCODE_E2E_SSH_WORKSPACE_PATHS 必须是两个不同的绝对路径`);
  }

  return {
    host,
    password,
    port,
    username,
    workspacePaths: [workspacePaths[0]!, workspacePaths[1]!],
  };
}

function requireSecretEnv(name: string): string {
  const value = process.env[name]?.trim();
  if (!value) {
    throw new Error(`${CASE_MARKER}: 缺少 ${name}`);
  }
  return value;
}

async function connectWorkspaceAndSendFirstTurn(options: {
  config: SSHRuntimeConfig;
  label: "A" | "B";
  provider: { id: string; name: string };
  workspacePath: string;
}): Promise<TaskCase> {
  // 第二个 workspace 从第一个已完成 task 进入；远程连接入口只在新任务草稿的
  // workspace 选择器中展示，因此先显式创建草稿，再走真实 UI 连接链路。
  await startNewV4Draft();
  await waitForDraftWorkspaceSettled();
  await openSSHRemoteDialog(CASE_MARKER);
  await fillSSHPasswordSettings(options.config);
  // S5-e 后「添加设备」连接成功即关闭向导，由项目选择器承接目录并创建项目。
  const tab = await completeSSHDeviceConnection({
    caseMarker: CASE_MARKER,
    workspacePath: options.workspacePath,
  });
  expect(tab.remoteKind).toBe("ssh");
  expect(tab.remoteHost).toBe(options.config.host);
  expect(tab.secretPersistedInTab).toBe(false);
  if (!tab.remoteSessionId) {
    throw new Error(`${CASE_MARKER}: workspace ${options.label} 缺少 remoteSessionId`);
  }
  if (!tab.workspaceIdentity) {
    throw new Error(`${CASE_MARKER}: workspace ${options.label} 缺少稳定 workspaceIdentity`);
  }

  await selectUpstreamProviderModelById(MODEL_ID, {
    includePlainModelFallback: false,
    providerId: options.provider.id,
    providerName: options.provider.name,
  });
  await startNewV4Draft();

  const firstPrompt = `${PROMPT_MARKER}_${options.label}_FIRST_PROMPT`;
  const firstReply = `${PROMPT_MARKER}_${options.label}_FIRST_REPLY`;
  const followPrompt = `${PROMPT_MARKER}_${options.label}_FOLLOW_PROMPT`;
  const followReply = `${PROMPT_MARKER}_${options.label}_FOLLOW_REPLY`;
  const requestStart = await getUpstreamRequestRecordCount();
  await sendV4PromptAndWaitAccepted(
    `${firstPrompt}: 只回复 ${firstReply}`,
    firstPrompt,
    `${options.label} 首发没有进入远端 task`,
  );
  const running = await waitForV4ConversationState(
    (snapshot) =>
      Boolean(snapshot.taskId && snapshot.taskId !== "draft") && snapshot.state === "streaming",
    `${options.label} 首发没有进入 running`,
    TURN_TIMEOUT_MS,
  );
  if (!running.taskId) {
    throw new Error(`${CASE_MARKER}: workspace ${options.label} 首发没有 taskId`);
  }
  const task: TaskCase = {
    firstPrompt,
    firstReply,
    followPrompt,
    followReply,
    identity: tab.workspaceIdentity,
    label: options.label,
    remoteSessionId: tab.remoteSessionId,
    taskId: running.taskId,
    workspacePath: options.workspacePath,
  };
  await assertTaskRow(task, { active: true, running: true });
  await waitForUpstreamRequest(
    {
      lastUserMessageIncludes: [firstPrompt],
    },
    `${options.label} 首发请求没有到达 provider`,
    TURN_TIMEOUT_MS,
    { afterIndex: requestStart - 1 },
  );
  await waitForV4AssistantMessageContaining(firstReply, TURN_TIMEOUT_MS);
  await waitForV4ConversationState(
    (snapshot) =>
      snapshot.taskId === task.taskId && snapshot.state === "idle" && snapshot.queueCount === 0,
    `${options.label} 首发完成后没有回到 idle`,
    TURN_TIMEOUT_MS,
  );
  await assertTaskRow(task, { active: true, running: false });
  return task;
}

async function waitForDraftWorkspaceSettled(): Promise<void> {
  let previousKey: string | null = null;
  let stablePolls = 0;
  await browser.waitUntil(
    async () => {
      const snapshot = await browser.execute((triggerTestId) => {
        const store = (
          window as Window & {
            __escodeTabStoreE2E?: {
              getState?: () => {
                activeTabId?: string | null;
                activeWorkspaceIdentity?: string | null;
                activeWorkspacePath?: string | null;
              };
            };
          }
        ).__escodeTabStoreE2E;
        const state = store?.getState?.();
        const visibleTriggerCount = Array.from(
          document.querySelectorAll<HTMLElement>(`[data-testid="${triggerTestId}"]`),
        ).filter((element) => element.getClientRects().length > 0).length;
        return {
          activeTabId: state?.activeTabId ?? null,
          activeWorkspaceIdentity: state?.activeWorkspaceIdentity ?? null,
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

async function switchTaskAndSendFollowTurn(
  task: TaskCase,
  foreignFirstPrompt: string,
): Promise<void> {
  await selectV4TaskById(task.taskId, 30_000);
  await waitForRemoteTab({
    expectedIdentity: task.identity,
    expectedPath: task.workspacePath,
    expectedRemoteSessionId: task.remoteSessionId,
  });
  await assertTaskRow(task, { active: true, running: false });
  const requestStart = await getUpstreamRequestRecordCount();
  await sendV4PromptAndWaitAccepted(
    `${task.followPrompt}: 只回复 ${task.followReply}`,
    task.followPrompt,
    `${task.label} 续发没有进入原 task`,
  );
  await assertTaskRow(task, { active: true, running: true });
  await waitForUpstreamRequest(
    {
      excludes: [foreignFirstPrompt],
      includes: [task.firstPrompt],
      lastUserMessageIncludes: [task.followPrompt],
    },
    `${task.label} 续发请求没有携带本 task 历史，或混入另一 workspace`,
    TURN_TIMEOUT_MS,
    { afterIndex: requestStart - 1 },
  );
  await waitForV4AssistantMessageContaining(task.followReply, TURN_TIMEOUT_MS);
  await waitForV4ConversationState(
    (snapshot) =>
      snapshot.taskId === task.taskId && snapshot.state === "idle" && snapshot.queueCount === 0,
    `${task.label} 续发完成后没有回到 idle`,
    TURN_TIMEOUT_MS,
  );
  await assertTaskRow(task, { active: true, running: false });
}

async function assertTaskRow(
  task: TaskCase,
  expected: { active: boolean; running: boolean },
): Promise<void> {
  let latest: {
    active: boolean;
    error: boolean;
    exists: boolean;
    inWorkspace: boolean;
    running: boolean;
  } | null = null;
  await browser.waitUntil(
    async () => {
      latest = await browser.execute(
        (taskTestId, workspaceTestId) => {
          const taskRow = Array.from(document.querySelectorAll<HTMLElement>("[data-testid]")).find(
            (element) => element.dataset.testid === taskTestId,
          );
          const workspaceRow = Array.from(
            document.querySelectorAll<HTMLElement>("[data-testid]"),
          ).find((element) => element.dataset.testid === workspaceTestId);
          const workspaceContainer = workspaceRow?.closest("li");
          return {
            active: taskRow?.classList.contains("bg-selected") ?? false,
            error: Boolean(taskRow?.querySelector('[data-error-indicator="true"]')),
            exists: Boolean(taskRow),
            inWorkspace: Boolean(taskRow && workspaceContainer?.contains(taskRow)),
            running: Boolean(taskRow?.querySelector(".animate-spin")),
          };
        },
        testId(TID_TASK_ITEM, task.taskId),
        testId(TID_WORKSPACE_ITEM, task.workspacePath),
      );
      return (
        latest.exists &&
        latest.inWorkspace &&
        latest.active === expected.active &&
        latest.running === expected.running &&
        !latest.error
      );
    },
    {
      timeout: 30_000,
      interval: 100,
      timeoutMsg: `${CASE_MARKER}: task ${task.label} 列表状态不正确 latest=${JSON.stringify(latest)}`,
    },
  );
}

async function createReplayTunnel(config: SSHRuntimeConfig): Promise<ReplayTunnel> {
  const replayBaseURL = await resolveSeededReplayBaseUrl();
  if (!replayBaseURL) {
    throw new Error(`${CASE_MARKER}: 没有找到本轮 replay provider URL`);
  }
  const replayURL = new URL(replayBaseURL);
  const replayPort = Number.parseInt(replayURL.port, 10);
  if (!Number.isSafeInteger(replayPort) || replayPort < 1) {
    throw new Error(`${CASE_MARKER}: replay provider URL 缺少有效端口`);
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
    socket.once("connect", () => {
      channel.pipe(socket).pipe(channel);
    });
    socket.once("close", () => channel.destroy());
    channel.once("close", () => socket.destroy());
  });

  const remotePort = await new Promise<number>((resolve, reject) => {
    client.once("error", reject);
    client.once("ready", () => {
      client.forwardIn("127.0.0.1", 0, (error, assignedPort) => {
        if (error) {
          reject(error);
          return;
        }
        resolve(assignedPort);
      });
    });
    client.connect({
      host: config.host,
      password: config.password,
      port: config.port,
      readyTimeout: CONNECT_TIMEOUT_MS,
      username: config.username,
    });
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

async function waitForRemoteTab(options: {
  expectedIdentity: string;
  expectedPath: string;
  expectedRemoteSessionId?: string | null;
  timeoutMs?: number;
}): Promise<RemoteTabSnapshot> {
  let snapshot: RemoteTabSnapshot | null = null;
  try {
    await browser.waitUntil(
      async () => {
        try {
          snapshot = await readRemoteTabSnapshot(options.expectedIdentity);
          return (
            snapshot.workspacePath === options.expectedPath &&
            snapshot.activeWorkspacePath === options.expectedPath &&
            snapshot.workspaceIdentity === options.expectedIdentity &&
            snapshot.activeWorkspaceIdentity === options.expectedIdentity &&
            Boolean(snapshot.remoteSessionId) &&
            (!options.expectedRemoteSessionId ||
              snapshot.remoteSessionId === options.expectedRemoteSessionId)
          );
        } catch {
          return false;
        }
      },
      {
        timeout: options.timeoutMs ?? WORKSPACE_READY_TIMEOUT_MS,
        interval: 1_000,
        timeoutMsg: `${CASE_MARKER}: remote tab identity/session 没有收敛`,
      },
    );
  } catch (error) {
    // Bug 定位需要区分“remote tab 丢失”和“reload 后错误恢复到 local active tab”；
    // snapshot 不包含凭据，可以安全进入 E2E 失败证据。
    throw new Error(
      `${CASE_MARKER}: remote tab identity/session 没有收敛，latest=${JSON.stringify(snapshot)}`,
      { cause: error },
    );
  }
  if (!snapshot) throw new Error(`${CASE_MARKER}: remote tab snapshot 缺失`);
  return snapshot;
}

function readRemoteTabSnapshot(expectedIdentity: string): Promise<RemoteTabSnapshot> {
  return browser.execute((identity) => {
    type RemoteTab = {
      id: string;
      kind: string;
      remoteSessionId?: string;
      remoteTarget?: Record<string, unknown>;
      workspaceIdentity?: string;
      workspacePath?: string;
    };
    const store = (
      window as Window & {
        __escodeTabStoreE2E?: {
          getState?: () => {
            activeTabId?: string | null;
            activeWorkspaceIdentity?: string | null;
            activeWorkspacePath?: string | null;
            tabs?: RemoteTab[];
          };
        };
      }
    ).__escodeTabStoreE2E;
    const state = store?.getState?.();
    const matchingRemoteTab = state?.tabs?.find((tab) => tab.workspaceIdentity === identity);
    const remoteTarget = matchingRemoteTab?.remoteTarget;
    return {
      activeWorkspaceIdentity: state?.activeWorkspaceIdentity ?? null,
      activeWorkspacePath: state?.activeWorkspacePath ?? null,
      remoteHost: typeof remoteTarget?.host === "string" ? remoteTarget.host : null,
      remoteKind: typeof remoteTarget?.kind === "string" ? remoteTarget.kind : null,
      remoteSessionId: matchingRemoteTab?.remoteSessionId ?? null,
      secretPersistedInTab: Boolean(
        remoteTarget && ("password" in remoteTarget || "privateKeyPassphrase" in remoteTarget),
      ),
      workspaceIdentity: matchingRemoteTab?.workspaceIdentity ?? null,
      workspacePath: matchingRemoteTab?.workspacePath ?? null,
    };
  }, expectedIdentity);
}

async function readHostProcesses(): Promise<Array<{ name: string; pid: number }>> {
  await openResourceManager();
  await switchToElectronRendererTarget(isResourceManagerUrl);
  await waitForResourceManagerReady();
  const hosts = await listHostProcessIdentities();
  await switchToElectronRendererTarget(isMainRendererUrl);
  return hosts;
}
