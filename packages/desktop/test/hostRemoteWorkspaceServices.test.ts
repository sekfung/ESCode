import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ChannelClient, ChannelServer, createQueuePair, Emitter, ProxyChannel } from "@escode/rpc";
import {
  ServiceCollection,
  IFileService,
  IGitService,
  IGitCheckpointService,
  ISystemService,
  ITerminalService,
  ISettingService,
  IOnboardingRecordService,
  ICredentialService,
  IBroadcastService,
  IESCodeAgentService,
  IESCodeSessionService,
  IESCodeTaskService,
  IFileWatcherService,
  IModelSelectionService,
  IProviderSettingsService,
  IUsageStatsService,
  IClientConfigService,
  ISkillsService,
  ISkillSyncService,
  IMcpSyncService,
  IPluginSyncService,
  IPluginsService,
  ISubagentsService,
  ICommandsService,
  ISettingsSyncService,
  type ESCodeAgentSessionRuntimePreferencesRequest,
} from "@escode/services";
import type { ApiClient } from "@escode/shared";
import { createRemoteConnectionServiceCollection } from "../src/host/remoteConnectionServiceCollection.js";
import { createRemotePromptAttachmentTaskService } from "../src/host/remotePromptAttachments.js";
import { setDataBaseDir } from "@escode/services/node";
import * as nodeServices from "@escode/services/node";
import {
  createRemoteWorkspaceServiceCollection,
  createServerRemoteWorkspaceServiceCollection,
} from "../src/host/remoteWorkspaceServiceCollection.js";

const originalHome = process.env.HOME;
const clientConfigService = {
  getSnapshot: vi.fn(async () => ({ pluginStoreOrder: null })),
};

function createTempHome(): string {
  const home = mkdtempSync(join(tmpdir(), "escode-remote-workspace-services-"));
  // Bug 根因：模型供应商存储的默认 data base dir 在模块加载时已经固定；只改 HOME
  // 会继续读取真实 ~/.escode/v2，并让 getAll 的后台权益请求污染下一个测试。
  setDataBaseDir(home);
  // Bug 根因：设置服务按 process.env.HOME 实时定位 ~/.escode/v2/setting.json。只改 data base dir
  // 时会读到开发机真实设置；配置了 httpProxy 的机器上 Host API 改走 undici 代理 dispatcher，
  // 绕过 globalThis.fetch mock 直连线上并 401。HOME 由 afterEach 统一还原。
  process.env.HOME = home;
  return home;
}

function createPresetFetchMock(modelContextBudgetStrategy: "legacy" | "preflight-v1" = "legacy") {
  return vi.fn(async (input: RequestInfo | URL) => {
    const url = String(input);
    if (url.includes("192.168.100.166") || url.includes("/v1/models")) {
      throw new Error(`Retired preset endpoints must not be requested remotely: ${url}`);
    }
    if (url.includes("/api/v2/releases/latest")) {
      return new Response(
        JSON.stringify({
          version: "0.25.1",
          config_version: "0.0.13",
        }),
        { status: 200, headers: { "content-type": "application/json" } },
      );
    }
    if (url.includes("/api/v1/client/configs")) {
      return new Response(
        JSON.stringify({
          code: 0,
          msg: "",
          data: {
            configs: {
              modelContextBudget: {
                strategy: modelContextBudgetStrategy,
              },
            },
          },
        }),
        { status: 200, headers: { "content-type": "application/json" } },
      );
    }
    return new Response(JSON.stringify({}), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  });
}

function makeRemoteServices() {
  const runtimePreferencesEmitter = new Emitter<ESCodeAgentSessionRuntimePreferencesRequest>();

  return {
    fileService: { remote: "file" },
    gitService: { remote: "git" },
    gitCheckpointService: { remote: "git-checkpoint" },
    systemService: { remote: "system" },
    terminalService: { remote: "terminal" },
    escodeAgentService: {
      onDynamicSessionRuntimePreferencesRequest: vi.fn(() => runtimePreferencesEmitter.event),
      respondSessionRuntimePreferences: vi.fn(async () => undefined),
    },
    runtimePreferencesEmitter,
    escodeTaskService: { sendPrompt: vi.fn() },
    escodeSessionService: { remote: "escode-session" },
    fileWatcherService: { remote: "file-watcher" },
    skillsService: { remote: "skills" },
    skillSyncService: { remote: "skill-sync" },
    mcpSyncService: { remote: "mcp-sync" },
    pluginSyncService: { remote: "plugin-sync" },
    pluginsService: { remote: "plugins" },
    pluginManagementService: { remote: "plugin-management" },
    commandsService: { remote: "commands" },
    hooksService: { remote: "hooks" },
    modelSelectionService: {
      getView: vi.fn(async () => ({ revision: 0, providers: [] })),
    },
    providerSettingsService: { remote: "provider-settings" },
  } as never;
}

function remoteWorkspaceHostOptions(onError = vi.fn()) {
  return {
    localOnboardingRecordService: createOnboardingRecordStub(),
    promptAttachmentTransferService: {} as never,
    runtimePreferencesBridge: {
      onError,
    },
  };
}

function createOnboardingRecordStub(): IOnboardingRecordService {
  return {
    appendRecord: vi.fn(async () => {}),
    dismissOnboarding: vi.fn(async () => {}),
    claimAnonymousRecord: vi.fn(async () => {}),
    shouldOnboard: vi.fn(async () => false),
    getLatestEntry: vi.fn(async () => null),
    syncSettingsFromRecord: vi.fn(async () => null),
    updateRecordPreferences: vi.fn(async () => {}),
    getRecords: vi.fn(async () => null),
    clearRecords: vi.fn(async () => {}),
  };
}

function passthroughService<T extends object>(service: T): T {
  return service;
}

describe("remote workspace host service wiring", () => {
  afterEach(() => {
    process.env.HOME = originalHome;
    setDataBaseDir(null);
    vi.restoreAllMocks();
  });

  it("keeps workspace services remote and exposes local global services for replayable attachment", () => {
    const remoteServices = makeRemoteServices();
    const services = createRemoteWorkspaceServiceCollection({
      clientConfigService,
      connectionServices: remoteServices,
      ...remoteWorkspaceHostOptions(),
      parentPort: null,
      createRemotePromptAttachmentTaskService: passthroughService,
      createRemotePromptAttachmentSessionService: passthroughService,
      createReportingRemoteESCodeTaskService: (service) => ({ wrapped: service }) as never,
    });

    expect(services.get(IFileService)).toBe(remoteServices.fileService);
    expect(services.get(IGitService)).toBe(remoteServices.gitService);
    expect(services.get(IGitCheckpointService)).toBe(remoteServices.gitCheckpointService);
    expect(services.get(ISystemService)).toBe(remoteServices.systemService);
    expect(services.get(ITerminalService)).toBe(remoteServices.terminalService);
    expect(services.get(IFileWatcherService)).toBe(remoteServices.fileWatcherService);
    expect(services.get(IESCodeTaskService)).toEqual({
      wrapped: remoteServices.escodeTaskService,
    });
    expect(services.get(IESCodeAgentService)).toBe(remoteServices.escodeAgentService);
    expect(services.get(IESCodeSessionService)).toBe(remoteServices.escodeSessionService);

    expect(services.get(IClientConfigService)).toBe(clientConfigService);
    expect(services.get(ISettingService)).toBeTruthy();
    expect(services.get(ICredentialService)).toBeTruthy();
    expect(services.get(IBroadcastService)).toBeTruthy();
    expect(services.get(IModelSelectionService)).toBe(remoteServices.modelSelectionService);
    expect(services.get(IProviderSettingsService)).toBe(remoteServices.providerSettingsService);
    expect(services.get(IUsageStatsService)).toBeTruthy();
    expect(services.get(IClientConfigService)).toBe(clientConfigService);
    expect(services.get(ISkillsService)).toBe(remoteServices.skillsService);
    expect(services.get(ISkillSyncService)).toBe(remoteServices.skillSyncService);
    expect(services.get(IMcpSyncService)).toBe(remoteServices.mcpSyncService);
    expect(services.get(IPluginSyncService)).toBe(remoteServices.pluginSyncService);
    expect(services.get(IPluginsService)).toBe(remoteServices.pluginsService);
    expect(services.get(ICommandsService)).toBe(remoteServices.commandsService);
    expect(services.get(ISubagentsService)).toBeTruthy();
    expect(services.get(ISettingsSyncService)).toBeTruthy();
  });

  it.each(["desktop-attached", "server"] as const)(
    "%s 远程 attachment 通过 RPC 访问同源引导记录并保留服务错误",
    async (kind) => {
      const localRecord = createOnboardingRecordStub();
      const serverRecord = createOnboardingRecordStub();
      const owner = kind === "server" ? serverRecord : localRecord;
      const other = kind === "server" ? localRecord : serverRecord;
      const connectionServices = {
        ...makeRemoteServices(),
        onboardingRecordService: serverRecord,
      };
      const sourceServices = new ServiceCollection().register(
        IOnboardingRecordService,
        localRecord,
      );
      const services =
        kind === "server"
          ? createServerRemoteWorkspaceServiceCollection({
              clientConfigService,
              connectionServices,
              sourceServices,
            })
          : createRemoteWorkspaceServiceCollection({
              clientConfigService,
              connectionServices,
              sourceServices,
              ...remoteWorkspaceHostOptions(),
              localOnboardingRecordService: localRecord,
              parentPort: null,
              createRemotePromptAttachmentTaskService: passthroughService,
              createRemotePromptAttachmentSessionService: passthroughService,
              createReportingRemoteESCodeTaskService: passthroughService,
            });
      const [serverProtocol, clientProtocol] = createQueuePair();
      const server = new ChannelServer(serverProtocol, "mobile-onboarding", 50);
      const client = new ChannelClient(clientProtocol);
      services.exposeOnChannelServer(server);
      const record = ProxyChannel.toService<IOnboardingRecordService>(
        client.getChannel(IOnboardingRecordService.channelName),
      );
      try {
        // 远程集合曾漏注册通道；即使 owner 已有记录，远程 workspace 也只能收到 Unknown channel。
        await record.claimAnonymousRecord();
        await expect(record.shouldOnboard("mobile-device")).resolves.toBe(false);
        await record.dismissOnboarding("mobile-device");
        const entry = {
          occupation: "developer",
          interfaceMode: "coding" as const,
          memoryEnabled: false,
          proactiveSuggestionsEnabled: false,
          completedAt: "2026-10-09T00:00:00.000Z",
        };
        await record.appendRecord("mobile-device", entry);
        expect(services.get(IOnboardingRecordService)).toBe(owner);
        expect(owner.dismissOnboarding).toHaveBeenCalledWith("mobile-device");
        expect(owner.appendRecord).toHaveBeenCalledWith("mobile-device", entry);
        expect(other.claimAnonymousRecord).not.toHaveBeenCalled();
        expect(other.shouldOnboard).not.toHaveBeenCalled();
        expect(other.dismissOnboarding).not.toHaveBeenCalled();
        expect(other.appendRecord).not.toHaveBeenCalled();
        vi.mocked(owner.shouldOnboard).mockRejectedValueOnce(new Error("record read failed"));
        await expect(record.shouldOnboard("mobile-device")).rejects.toThrow("record read failed");
      } finally {
        client.dispose();
        server.dispose();
      }
    },
  );

  it("bridges app-global runtime preferences without a workspace-scoped subscription", async () => {
    const home = createTempHome();
    vi.spyOn(globalThis, "fetch").mockImplementation(createPresetFetchMock("preflight-v1"));
    const remoteServices = makeRemoteServices();
    const services = createRemoteWorkspaceServiceCollection({
      clientConfigService,
      connectionServices: remoteServices,
      ...remoteWorkspaceHostOptions(),
      parentPort: null,
      createRemotePromptAttachmentTaskService: passthroughService,
      createRemotePromptAttachmentSessionService: passthroughService,
      createReportingRemoteESCodeTaskService: (service) => service,
    });
    try {
      await services.get(ISettingService).update({
        askUserQuestionAutoResolutionEnabled: false,
        nativeSearchEnhancementsEnabled: false,
        integratedTerminalShell: {
          mode: "shell",
          dialect: "git-bash",
          id: "git-bash:C:/Program Files/Git/bin/bash.exe",
          label: "Git Bash",
          path: "C:/Program Files/Git/bin/bash.exe",
        },
      });
      remoteServices.runtimePreferencesEmitter.fire({
        requestId: "request-runtime-preferences",
        sessionId: "session-remote",
        scope: "runtime-materialization",
      });

      await vi.waitFor(() => {
        expect(
          remoteServices.escodeAgentService.onDynamicSessionRuntimePreferencesRequest,
        ).toHaveBeenCalledWith();
        expect(
          remoteServices.escodeAgentService.respondSessionRuntimePreferences,
        ).toHaveBeenCalledWith({
          requestId: "request-runtime-preferences",
          resolution: {
            status: "resolved",
            preferences: {
              askUserQuestionAutoResolutionEnabled: false,
              nativeSearchEnhancementsEnabled: false,
              memoryEnabled: false,
              modelContextBudgetStrategy: "preflight-v1",
            },
          },
        });
      });
      remoteServices.runtimePreferencesEmitter.fire({
        requestId: "request-user-execution-preferences",
        sessionId: "session-remote",
        scope: "user-execution",
      });

      await vi.waitFor(() => {
        expect(
          remoteServices.escodeAgentService.respondSessionRuntimePreferences,
        ).toHaveBeenCalledWith({
          requestId: "request-user-execution-preferences",
          resolution: {
            status: "resolved",
            preferences: {
              askUserQuestionAutoResolutionEnabled: false,
              nativeSearchEnhancementsEnabled: false,
              memoryEnabled: false,
              modelContextBudgetStrategy: "preflight-v1",
              integratedTerminalShell: {
                mode: "shell",
                dialect: "git-bash",
                id: "git-bash:C:/Program Files/Git/bin/bash.exe",
                label: "Git Bash",
                path: "C:/Program Files/Git/bin/bash.exe",
              },
            },
          },
        });
      });
      // 套餐族删除（2026-10-15）：modelContextBudgetStrategy 的远端来源随订阅服务移除，
      // 响应固定 preflight-v1，不再存在可等待/可挂起的 strategy 解析源。
    } finally {
      rmSync(home, { recursive: true, force: true });
    }
  });

  it("reports a runtime preference response transport failure without retrying", async () => {
    const home = createTempHome();
    vi.spyOn(globalThis, "fetch").mockImplementation(createPresetFetchMock());
    const remoteServices = makeRemoteServices();
    const transportError = new Error("transport closed");
    const onError = vi.fn();
    remoteServices.escodeAgentService.respondSessionRuntimePreferences.mockRejectedValueOnce(
      transportError,
    );
    const services = createRemoteWorkspaceServiceCollection({
      clientConfigService,
      connectionServices: remoteServices,
      ...remoteWorkspaceHostOptions(onError),
      parentPort: null,
      createRemotePromptAttachmentTaskService: passthroughService,
      createRemotePromptAttachmentSessionService: passthroughService,
      createReportingRemoteESCodeTaskService: (service) => service,
    });

    try {
      await services.get(ISettingService).update({
        askUserQuestionAutoResolutionEnabled: false,
        nativeSearchEnhancementsEnabled: false,
        memoryEnabled: false,
      });
      remoteServices.runtimePreferencesEmitter.fire({
        requestId: "request-closed-transport",
        sessionId: "session-remote",
        scope: "runtime-materialization",
      });

      await vi.waitFor(() => {
        expect(onError).toHaveBeenCalledOnce();
      });
      expect(onError).toHaveBeenCalledWith(transportError);
      expect(
        remoteServices.escodeAgentService.respondSessionRuntimePreferences,
      ).toHaveBeenCalledOnce();
      expect(
        remoteServices.escodeAgentService.respondSessionRuntimePreferences,
      ).toHaveBeenCalledWith({
        requestId: "request-closed-transport",
        resolution: {
          status: "resolved",
          preferences: {
            askUserQuestionAutoResolutionEnabled: false,
            nativeSearchEnhancementsEnabled: false,
            memoryEnabled: false,
            modelContextBudgetStrategy: "preflight-v1",
          },
        },
      });
    } finally {
      rmSync(home, { recursive: true, force: true });
    }
  });

  it("does not warm legacy model provider presets when constructing remote workspace services", async () => {
    const home = createTempHome();
    const fetchImpl = createPresetFetchMock();
    vi.spyOn(globalThis, "fetch").mockImplementation(fetchImpl);
    const remoteServices = makeRemoteServices();
    createRemoteWorkspaceServiceCollection({
      clientConfigService,
      connectionServices: remoteServices,
      ...remoteWorkspaceHostOptions(),
      parentPort: null,
      createRemotePromptAttachmentTaskService: passthroughService,
      createRemotePromptAttachmentSessionService: passthroughService,
      createReportingRemoteESCodeTaskService: (service) => service,
    });

    try {
      await new Promise((resolve) => setTimeout(resolve, 50));

      expect(fetchImpl).not.toHaveBeenCalled();
    } finally {
      rmSync(home, { recursive: true, force: true });
    }
  });

  it("exposes remote skill sync service in the remote connection host", () => {
    const remoteServices = makeRemoteServices();
    const credentialService = {
      load: vi.fn(),
      save: vi.fn(),
      delete: vi.fn(),
    };
    const services = createRemoteConnectionServiceCollection({
      clientConfigService,
      connectionServices: remoteServices,
      parentPort: null,
      credentialService,
      apiClient: { request: vi.fn() } as unknown as ApiClient,
      remoteESCodeTaskService: { remote: "wrapped-task" } as never,
      remoteESCodeSessionService: { remote: "wrapped-session" } as never,
    });

    expect(services.get(IClientConfigService)).toBe(clientConfigService);
    expect(services.get(ISkillsService)).toBe(remoteServices.skillsService);
    expect(services.get(ISkillSyncService)).toBe(remoteServices.skillSyncService);
    expect(services.get(IMcpSyncService)).toBe(remoteServices.mcpSyncService);
    expect(services.get(IPluginSyncService)).toBe(remoteServices.pluginSyncService);
    expect(services.get(IPluginsService)).toBe(remoteServices.pluginsService);
    expect(services.get(ICommandsService)).toBe(remoteServices.commandsService);
  });

  it("keeps server remote workspace runtime services server-authoritative", () => {
    const remoteServices = {
      ...makeRemoteServices(),
      settingService: { remote: "setting" },
      onboardingRecordService: createOnboardingRecordStub(),
      credentialService: { remote: "credential" },
      broadcastService: { remote: "broadcast" },
      oauthService: { remote: "oauth" },
      usageStatsService: { remote: "usage-stats" },
      codingPlanSubscriptionService: { remote: "coding-plan" },
      pluginManagementService: { remote: "plugin-management" },
      subagentsService: { remote: "subagents" },
      hooksService: { remote: "hooks" },
      memoryService: { remote: "memory" },
      outputStyleService: { remote: "output-style" },
      settingsSyncService: { remote: "settings-sync" },
    } as never;

    const services = createServerRemoteWorkspaceServiceCollection({
      clientConfigService,
      connectionServices: remoteServices,
    });

    expect(services.get(IClientConfigService)).toBe(clientConfigService);
    expect(services.get(ISettingService)).toBe(remoteServices.settingService);
    expect(services.get(IOnboardingRecordService)).toBe(remoteServices.onboardingRecordService);
    expect(services.get(ICredentialService)).toBe(remoteServices.credentialService);
    expect(services.get(IBroadcastService)).toBe(remoteServices.broadcastService);
    expect(services.get(IModelSelectionService)).toBe(remoteServices.modelSelectionService);
    expect(services.get(IProviderSettingsService)).toBe(remoteServices.providerSettingsService);
    expect(services.get(IUsageStatsService)).toBe(remoteServices.usageStatsService);
    expect(services.get(ISubagentsService)).toBe(remoteServices.subagentsService);
    expect(services.get(ISettingsSyncService)).toBe(remoteServices.settingsSyncService);
  });

  it("materializes replayable task prompts before the reporting wrapper observes them", async () => {
    const hostAttachment = {
      kind: "file",
      filename: "pasted.txt",
      localPath: "/tmp/host-paste.txt",
      mimeType: "text/plain",
      sizeBytes: 5,
      sourceKind: "clipboard-text",
    } as const;
    const remoteAttachment = {
      ...hostAttachment,
      localPath: "/home/tester/.escode/tmp/prompt-attachments/input-1/01-pasted.txt",
    };
    const rawSendPrompt = vi.fn(async () => undefined);
    const observedByReporting: unknown[] = [];
    const remoteServices = {
      ...makeRemoteServices(),
      escodeTaskService: { sendPrompt: rawSendPrompt },
    } as never;
    const services = createRemoteWorkspaceServiceCollection({
      clientConfigService,
      connectionServices: remoteServices,
      ...remoteWorkspaceHostOptions(),
      parentPort: null,
      createRemotePromptAttachmentTaskService: (service) =>
        createRemotePromptAttachmentTaskService(service, {
          materializePromptAttachments: async (params) => ({
            content: params.content.replace("/tmp/host-paste.txt", remoteAttachment.localPath),
            attachments: [remoteAttachment],
          }),
        }),
      createRemotePromptAttachmentSessionService: passthroughService,
      createReportingRemoteESCodeTaskService: (service) =>
        new Proxy(service, {
          get(target, property, receiver) {
            const value = Reflect.get(target, property, receiver);
            if (property !== "sendPrompt" || typeof value !== "function") {
              return value;
            }
            return async (...args: unknown[]) => {
              observedByReporting.push(args[0]);
              return value.apply(target, args);
            };
          },
        }) as never,
    });

    await services.get(IESCodeTaskService).sendPrompt({
      taskId: "task-1",
      traceId: "input-1",
      content: "read /tmp/host-paste.txt",
      attachments: [hostAttachment],
    });

    const expectedParams = {
      taskId: "task-1",
      traceId: "input-1",
      content: "read /home/tester/.escode/tmp/prompt-attachments/input-1/01-pasted.txt",
      attachments: [remoteAttachment],
    };
    expect(observedByReporting).toEqual([expectedParams]);
    expect(rawSendPrompt).toHaveBeenCalledWith(expectedParams);
  });
});
