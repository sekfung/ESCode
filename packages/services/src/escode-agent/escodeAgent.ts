import type { BackgroundBashOutputResult, SessionDebugSnapshot } from "@escode/shared";
/* eslint-disable max-lines -- ESCode agent service 接口集中声明 protocol/session/workspace 方法，拆分会增加 service descriptor 迁移成本。 */
import type { Event, IDisposable } from "@escode/rpc";
import { ServiceChannels } from "@escode/shared";
import type { AppUsageRange, AppUsageSnapshot, ESCodeTaskTokenUsageResult } from "@escode/shared";
import type { ESCodeAutomation, ESCodeAutomationRun } from "@escode/shared";
import type {
  ESCodeStorageStartupState,
  ESCodeDeliveryKind,
  ESCodeAgentMcpServer,
  ESCodeBackgroundTurnAttribution,
  TraceId,
  ESCodeSessionCompactResult,
  ESCodeSessionGoalAction,
  ESCodeSessionGoalResult,
  ESCodeMessageWithParts,
  ModelSelection,
  ESCodeSessionImportHistory,
  ESCodePermissionRequestParams,
  AgentLaneResourceSample,
  ESCodeMcpTelemetryEvent,
  ESCodeMcpResourceSample,
  ESCodeToolExecResource,
  ESCodeProcessChildProcess,
  ESCodeMcpListResult,
  ESCodePluginsListResult,
  ESCodePluginsOverviewResult,
  ESCodePluginsMarketplaceMutationResult,
  ESCodePluginsInstallResult,
  ESCodePluginsReferenceCatalogResult,
  ESCodeSkillsReferenceCatalogResult,
  ESCodeWorkflowsDeleteResult,
  ESCodeWorkflowsGetResult,
  ESCodeWorkflowsListResult,
  ESCodeWorkflowsMoveResult,
  ESCodeWorkflowsRunsResult,
  ESCodeWorkflowsUpdateMetaResult,
  ESCodePluginsUninstallResult,
  ESCodePluginsRestoreBuiltinResult,
  ESCodePluginsConfigureResult,
  ESCodePluginsDescribeResult,
  ESCodePluginsValidateResult,
  ESCodePluginsSetEnabledResult,
  ESCodePluginsCancelOperationResult,
  ESCodePluginOperationProgressNotification,
  ESCodeProviderTestModelConnectivityParams,
  ESCodeProviderTestModelConnectivityResult,
  ESCodeUserInputRequestParams,
  ESCodeUserInputResponse,
  ESCodeSessionEvent,
  ESCodeSessionInfo,
  ESCodeSessionMode,
  ESCodeSessionPersistence,
  ESCodeSessionSendResult,
  ESCodeSessionRequestRuntimePreferencesParams,
  ESCodeSessionRuntimePreferencesResult,
  ESCodeSessionStateSnapshot,
  ESCodeSessionSubagentsResult,
  ESCodeStateUpdatedNotification,
  ESCodeTaskClientMode,
  ESCodeBrowserAmbientContext,
  ESCodeWorkspacePresentation,
  ESCodeWorkspaceGenerateTextResult,
  ESCodeWorkspaceGenerateTextParams,
  ESCodeWorkspaceHookTrustGrantResult,
  ESCodeAutomationBotDeliveryTarget,
} from "@escode/shared";
import type {
  ClientHello,
  CommandAck,
  CommandEnvelope,
  CommandKey,
  CommandsQueryResult,
  ConversationTopicWireCandidate,
  ConversationTelemetryFact,
  CuaPermissionObservation,
  ConversationRowTarget,
  HelloMessage,
  SessionsIndexTopicWireCandidate,
  V4AttachmentBeginResult,
  V4AttachmentChunkResult,
  V4AttachmentCommitResult,
  V4AttachmentPreviewSourceResult,
  V4AttachmentReadResult,
  V4ConversationAttachmentReadResult,
  V4ConversationAttachmentStatResult,
  V4ConnectionFlowState,
  V4ConversationFileChangesResult,
  V4ConversationFileRewindPreviewResult,
  V4ConversationPlansResult,
  V4ConversationWorkflowRunEventsResult,
  V4ConversationWorkflowRunArtifactDataResult,
  V4ConversationWorkflowRunArtifactReadResult,
  V4ConversationWorkflowRunArtifactsResult,
  V4ConversationWorkflowRunNodeResultResult,
  V4ConversationWorkflowRunWorkspaceResult,
  V4ConversationWorkflowRunsResult,
  V4ConversationRowsRangeResult,
  V4ConversationResyncResult,
  V4ConversationSubscribeResult,
  V4SessionsIndexSubscribeResult,
  V4WorkspaceConfigSubscribeResult,
  WorkspaceConfigTopicWireCandidate,
} from "@escode/shared/escode-protocol-v4";
import { createServiceDescriptor } from "../descriptors.js";

export * from "./escodeAgentPluginParams.js";
export * from "./escodeAgentWorkflowParams.js";
import type {
  ESCodeAgentAddPluginMarketplaceParams,
  ESCodeAgentAutomationIdParams,
  ESCodeAgentCancelPluginOperationParams,
  ESCodeAgentConfigurePluginParams,
  ESCodeAgentResetPluginConfigParams,
  ESCodeAgentCreateAutomationParams,
  ESCodeAgentDeleteAutomationRunParams,
  ESCodeAgentDescribePluginParams,
  ESCodeAgentInstallPluginParams,
  ESCodeAgentListMcpServerStatusesParams,
  ESCodeAgentPluginViewParams,
  ESCodeAgentPluginReferenceCatalogParams,
  ESCodeAgentSkillReferenceCatalogParams,
  ESCodeAgentResolveSuggestedPluginReferenceParams,
  ESCodeAgentRemovePluginMarketplaceParams,
  ESCodeAgentRestoreBuiltinPluginParams,
  ESCodeAgentSetPluginEnabledParams,
  ESCodeAgentSetAutomationEnabledParams,
  ESCodeAgentUninstallPluginParams,
  ESCodeAgentUpdatePluginMarketplaceParams,
  ESCodeAgentUpdatePluginParams,
  ESCodeAgentUpdateAutomationParams,
  ESCodeAgentValidatePluginParams,
  ESCodeAgentWorkspaceTarget,
} from "./escodeAgentPluginParams.js";
import type {
  ESCodeAgentDeleteSavedWorkflowParams,
  ESCodeAgentGetSavedWorkflowParams,
  ESCodeAgentListSavedWorkflowRunsParams,
  ESCodeAgentListSavedWorkflowsParams,
  ESCodeAgentMoveSavedWorkflowParams,
  ESCodeAgentUpdateSavedWorkflowMetaParams,
} from "./escodeAgentWorkflowParams.js";

export interface ESCodeAgentSessionTarget extends ESCodeAgentWorkspaceTarget {
  sessionId: string;
}

export interface ESCodeAgentResumeSessionParams extends ESCodeAgentSessionTarget {
  model?: ModelSelection;
  thoughtLevel?: string;
  mcpServers?: ESCodeAgentMcpServer[];
  // 冷恢复会重建 runtime，工具面隔离必须和 create 保持同一安全边界（CUA 只放行 escode-cua 工具、
  // 禁 Bash 等）。否则 resume 后模型可见工具面/执行权限会比创建时更宽。
  toolAllowlist?: string[];
  toolDenylist?: string[];
}

export interface ESCodeAgentInitializeResult {
  available: boolean;
  workspaceKey: string;
  protocolName?: string;
  protocolVersion?: number;
  transportKind?: "stdio" | "websocket";
  reason?: string;
  reasonCode?: "provider_not_ready";
}

export interface ESCodeAgentRunAutomationNowResult {
  status: "queued" | "duplicate";
}

export interface ESCodeAgentWorkspaceRuntimeIdentity {
  generation: number;
  identity: string;
  processId?: number;
  workspaceKey: string;
}

export const ESCODE_AGENT_RUNTIME_UNAVAILABLE_CODE = "ESCODE_AGENT_RUNTIME_UNAVAILABLE";

export type ESCodeAgentRuntimePolicy = "start-if-needed" | "existing-only";

export interface ESCodeAgentRuntimeLifecycleEvent extends ESCodeAgentWorkspaceTarget {
  workspaceKey: string;
  runtimeIdentity: ESCodeAgentWorkspaceRuntimeIdentity;
  state: "available" | "unavailable";
}

export type ESCodeAgentCuaPermissionObservation = CuaPermissionObservation &
  ESCodeAgentWorkspaceTarget;

export interface ESCodeAgentCreateSessionParams extends ESCodeAgentWorkspaceTarget {
  sessionId?: string;
  sessionTraceId?: TraceId;
  parentSessionId?: string;
  mode?: ESCodeSessionMode;
  model?: ModelSelection;
  persistence?: ESCodeSessionPersistence;
  thoughtLevel?: string;
  /** automation 执行会话关闭模型二次命名，保持首条用户 query 作为稳定标题。 */
  titleGenerationEnabled?: boolean;
  mcpServers?: ESCodeAgentMcpServer[];
  toolAllowlist?: string[];
  toolDenylist?: string[];
  importedHistory?: ESCodeSessionImportHistory;
}

export interface ESCodeAgentListSessionsParams extends ESCodeAgentWorkspaceTarget {
  sessionIds?: string[];
  runtimePolicy?: ESCodeAgentRuntimePolicy;
  includeArchived?: boolean;
  limit?: number;
}

export interface ESCodeAgentListSessionSubagentsParams extends ESCodeAgentSessionTarget {
  endedCursor?: string;
  endedLimit?: number;
  /** 远程 workspace 的宿主连接身份；只用于选择现有 Host，不进入 CLI wire query。 */
  remoteSessionId?: string;
}

export interface ESCodeAgentAppUsageParams {
  range: AppUsageRange;
  timeZone?: string;
}

export interface ESCodeAgentTaskTokenUsageParams extends ESCodeAgentSessionTarget {}

export interface ESCodeAgentReadSessionParams extends ESCodeAgentSessionTarget {
  deliveryKind?: ESCodeDeliveryKind;
  messageLimit?: number;
  afterSeq?: number;
  /** 被动索引/观察者只能读取现有 runtime，禁止为了读快照拉起 session。 */
  runtimePolicy?: ESCodeAgentRuntimePolicy;
}

export interface ESCodeAgentReadSessionMessagesParams extends ESCodeAgentSessionTarget {
  afterMessageId?: string;
  limit?: number;
}

export interface ESCodeAgentReadSessionEventsParams extends ESCodeAgentSessionTarget {
  afterSeq?: number;
  limit?: number;
}

export type ESCodeAgentReadWorkspacePresentationParams = ESCodeAgentWorkspaceTarget;

export interface ESCodeAgentGrantWorkspaceHookTrustParams extends ESCodeAgentWorkspaceTarget {
  bundleDigest: string;
  hookDeclarationDigest: string;
}

export interface ESCodeAgentSendPromptParamsBase extends ESCodeAgentSessionTarget {
  modelSelection?: ModelSelection;
  modelExecution?: import("@escode/shared/escode-protocol-v4").CommandPayloadMap["sendText"]["modelExecution"];
  inputId?: string;
  queryId?: string;
  messageId?: string;
  sessionTraceId?: TraceId;
  content: string;
  attachments?: Record<string, unknown>[];
  /** provider-only 的当前 IAB 状态；UI/session persistence 仍使用 content 原文。 */
  browserAmbientContext?: ESCodeBrowserAmbientContext;
  clientMode?: ESCodeTaskClientMode;
  expectedRevision?: number;
  expectedProviderRevision?: string;
  runtimeProviderHeaders?: Record<string, string>;
  toolDenylist?: string[];
  /** Bot 来源 turn 的稳定回推地址；只在当前 turn 内供 CronCreate 读取。 */
  botDeliveryTarget?: ESCodeAutomationBotDeliveryTarget;
}

export type ESCodeAgentSendPromptParams = ESCodeAgentSendPromptParamsBase &
  ESCodeBackgroundTurnAttribution;

export interface ESCodeAgentCompactParams extends ESCodeAgentSessionTarget {
  inputId?: string;
  instructions?: string;
  expectedRevision?: number;
}

export interface ESCodeAgentGoalParams extends ESCodeAgentSessionTarget {
  inputId?: string;
  action: ESCodeSessionGoalAction;
  objective?: string;
  expectedRevision?: number;
}

export interface ESCodeAgentSetModelParams extends ESCodeAgentSessionTarget {
  model: ModelSelection;
  expectedRevision?: number;
  persistAsWorkspaceLastUsed?: boolean;
}

export interface ESCodeAgentSetThoughtLevelParams extends ESCodeAgentSessionTarget {
  thoughtLevel?: string;
  expectedRevision?: number;
  persistAsWorkspaceLastUsed?: boolean;
}

export interface ESCodeAgentSetModeParams extends ESCodeAgentSessionTarget {
  mode: ESCodeSessionMode;
  expectedRevision?: number;
}

export interface ESCodeAgentGenerateWorkspaceTextParams extends ESCodeAgentWorkspaceTarget {
  selection: ESCodeWorkspaceGenerateTextParams["selection"];
  prompt?: string;
  messages?: ESCodeWorkspaceGenerateTextParams["messages"];
  tools?: ESCodeWorkspaceGenerateTextParams["tools"];
  querySource: string;
  maxOutputTokens?: number;
  signal?: AbortSignal;
  /**
   * 协议层 RPC 超时。thinking 模型的长请求会超过协议 client 默认的
   * 3 分钟；调用方必须把自身 deadline 透传到这里，否则默认超时先触发、
   * 还会被 onRequestTimeout 误判 stale 杀进程。
   */
  requestTimeoutMs?: number;
}

export interface ESCodeAgentTestModelConnectivityParams extends ESCodeAgentWorkspaceTarget {
  selection: ESCodeProviderTestModelConnectivityParams["selection"];
  signal?: AbortSignal;
}

export interface ESCodeAgentSessionRuntimePreferencesRequest extends ESCodeSessionRequestRuntimePreferencesParams {
  requestId: string;
}

export interface ESCodeAgentRespondSessionRuntimePreferencesParams {
  requestId: string;
  resolution:
    | { status: "resolved"; preferences: ESCodeSessionRuntimePreferencesResult }
    | { status: "failed"; message: string };
}

export interface ESCodeAgentSessionSubscribeParams extends ESCodeAgentSessionTarget {
  deliveryKind: ESCodeDeliveryKind;
  afterSeq?: number;
  includeSnapshot?: boolean;
  eventCoalescing?: {
    mode: "background-summary";
    intervalMs?: number;
  };
}

// ── v4 conversation 通道（竖切）──
// host 只做转发：subscribe/unsubscribe/command 透传给 CLI v4 gateway，
// v4/conversation/frame 通知按 workspace fan-out 给 renderer。

export interface ESCodeAgentConversationSubscribeParams extends ESCodeAgentSessionTarget {
  /** 水位不变量：仅当客户端真持有该时刻一致状态才允许带。 */
  base?: { logEpoch: string; seq: number };
  visibility?: "foreground" | "background";
}

export interface ESCodeAgentConversationUnsubscribeParams extends ESCodeAgentWorkspaceTarget {
  subscriptionId: string;
  runtimePolicy?: ESCodeAgentRuntimePolicy;
}

export interface ESCodeAgentConversationResyncParams extends ESCodeAgentWorkspaceTarget {
  subscriptionId: string;
  base: { logEpoch: string; seq: number } | null;
  forceSnapshot?: boolean;
  runtimePolicy?: ESCodeAgentRuntimePolicy;
}

/** 行分页 query（rows/range）：按游标向上取一窗历史行。 */
export interface ESCodeAgentConversationRowsRangeParams extends ESCodeAgentSessionTarget {
  /** 取 rowId < beforeRowId 的行；缺省 = 从当前尾部向前。 */
  beforeRowId?: number;
  /** 1..rowsRangeMaxLimit（200）。 */
  limit: number;
}

/** 当前有效分支里的终态 ExitPlanMode 目录。 */
export type ESCodeAgentConversationPlansParams = ESCodeAgentSessionTarget;

/** workflow run 的事件日志分页（详情页审计面）；cursor = journal sequence。 */
export interface ESCodeAgentConversationWorkflowRunEventsParams extends ESCodeAgentSessionTarget {
  runId: string;
  afterSequence?: number;
  limit?: number;
}

/** dwf run 的枚举（重启后的发现查询）。 */
export interface ESCodeAgentConversationWorkflowRunsParams extends ESCodeAgentSessionTarget {
  limit?: number;
}

// ── dwf 用户面产物──
// ⚠ 术语：artifact = 脚本经 `artifact.*` 发布给**用户**看的产出（文件 / markdown / 预置看板），
// 不是 run 的顶层返回值（引擎内部对后者的同名叫法）。

/** 产物清单；UI 冷恢复与中枢详情的 durable 读法。 */
export interface ESCodeAgentConversationWorkflowRunArtifactsParams extends ESCodeAgentSessionTarget {
  runId: string;
}

/** 预置看板的取数面；cursor = journal sequence（严格大于）。 */
export interface ESCodeAgentConversationWorkflowRunArtifactDataParams extends ESCodeAgentSessionTarget {
  runId: string;
  artifactId: string;
  afterSequence?: number;
  limit?: number;
}

/** 内容产物的字节，一次一块（≤ 512 KiB，形状逐字照 attachmentRead）。 */
export interface ESCodeAgentConversationWorkflowRunArtifactReadParams extends ESCodeAgentSessionTarget {
  runId: string;
  artifactId: string;
  version: number;
  offset: number;
  limit: number;
}

// ── dwf 工作区 transcript──
/** 轻行清单：一个 run 的 files.* / git.* / world.run 行，不带正文。 */
export interface ESCodeAgentConversationWorkflowRunWorkspaceParams extends ESCodeAgentSessionTarget {
  runId: string;
}

/** 一个工作区节点的正文，按 maxBytes 保形有界化（缺省与上限在 CLI 网关侧）。 */
export interface ESCodeAgentConversationWorkflowRunNodeResultParams extends ESCodeAgentSessionTarget {
  runId: string;
  siteId: string;
  ordinal: number;
  maxBytes?: number;
}

export interface ESCodeAgentBackgroundBashOutputParams extends ESCodeAgentSessionTarget {
  workId: string;
}

export interface ESCodeAgentConversationFileChangesParams extends ESCodeAgentSessionTarget {
  target: ConversationRowTarget;
  baseRevision: number;
  baseLogEpoch: string;
}

export interface ESCodeAgentConversationFileRewindPreviewParams extends ESCodeAgentSessionTarget {
  target: ConversationRowTarget;
  baseRevision: number;
  baseLogEpoch: string;
}

export interface ESCodeAgentConversationCommandParams extends ESCodeAgentWorkspaceTarget {
  envelope: CommandEnvelope;
  /** 仅 host 内部用于 Browser Use runtime 边界，不进入 v4 wire envelope。 */
  clientMode?: ESCodeTaskClientMode;
}

export interface ESCodeAgentCommandsQueryParams extends ESCodeAgentWorkspaceTarget {
  clock?: true;
  commands: CommandKey[];
}

/** UI 不携带 connectionId；connection scope 以 trusted carrier 注入 wire identity。 */
export interface ESCodeAgentAttachmentBeginParams extends ESCodeAgentSessionTarget {
  uploadId: string;
  fileName: string;
  mime: string;
  totalBytes: number;
  totalChunks: number;
  checksum: string;
}

export interface ESCodeAgentAttachmentChunkParams extends ESCodeAgentSessionTarget {
  uploadId: string;
  chunkIndex: number;
  dataBase64: string;
}

export interface ESCodeAgentAttachmentTerminalParams extends ESCodeAgentSessionTarget {
  uploadId: string;
}

export interface ESCodeAgentAttachmentReadParams extends ESCodeAgentSessionTarget {
  ref: string;
  target?: ConversationRowTarget;
  attachmentIndex?: number;
  offset: number;
  limit: number;
}

export interface ESCodeAgentConversationAttachmentReadParams extends ESCodeAgentSessionTarget {
  ref: string;
  target: ConversationRowTarget;
  attachmentIndex: number;
  offset: number;
  limit: number;
}

export interface ESCodeAgentConversationAttachmentStatParams extends ESCodeAgentSessionTarget {
  ref: string;
  target: ConversationRowTarget;
  attachmentIndex: number;
}

export interface ESCodeAgentAttachmentPreviewSourceParams extends ESCodeAgentSessionTarget {
  ref: string;
  target?: ConversationRowTarget;
  attachmentIndex?: number;
}

/** host scope 内部 transport 控制面；connectionId 只能经 trusted carrier 注入。 */
export interface ESCodeAgentConnectionFlowParams extends ESCodeAgentWorkspaceTarget {
  state: V4ConnectionFlowState;
}

/** sessions-index：workspace 级列表订阅（无 sessionId 维度）。 */
export interface ESCodeAgentSessionsIndexSubscribeParams extends ESCodeAgentWorkspaceTarget {
  base?: { logEpoch: string; seq: number };
  visibility?: "foreground" | "background";
  /**
   * 订阅者作用域后缀：CLI 侧重订阅替换按 (connectionId, topic) 判定，
   * host 进程内多个独立消费者（renderer 侧栏 / task-index syncer）订阅同一 topic 时
   * 必须用不同 connectionId，否则互相替换对方的订阅代际。缺省共享 host 连接 id。
   */
  subscriberScope?: string;
  /**
   * task-list 等被动观察者必须使用 existing-only；runtime 不存在时返回稳定 unavailable，
   * 禁止为了建立列表订阅而启动 Agent。缺省保持显式会话入口的旧行为。
   */
  runtimePolicy?: ESCodeAgentRuntimePolicy;
}

/** workspace-config：workspace 级配置目录订阅（config options + slash 目录）。 */
export interface ESCodeAgentWorkspaceConfigSubscribeParams extends ESCodeAgentWorkspaceTarget {
  base?: { logEpoch: string; seq: number };
  visibility?: "foreground" | "background";
  subscriberScope?: string;
  runtimePolicy?: ESCodeAgentRuntimePolicy;
}

export type ESCodeAgentServiceEvent =
  | { type: "session.event"; event: ESCodeSessionEvent }
  | { type: "state.updated"; notification: ESCodeStateUpdatedNotification }
  | { type: "permission.request"; request: ESCodePermissionRequestParams }
  | { type: "userInput.request"; request: ESCodeUserInputRequestParams }
  | {
      type: "userInput.response";
      requestId: string;
      response: ESCodeUserInputResponse;
    }
  | { type: "snapshot"; snapshot: ESCodeSessionStateSnapshot };

export interface ESCodeAgentAppRuntimePreferences {
  askUserQuestionAutoResolutionEnabled: boolean;
  modelIoFullRetentionEnabled?: boolean;
}

export interface ESCodeAgentLocalRuntimeChildProcesses {
  pid: number;
  provider: string;
  workspacePath: string;
  lane?: string;
  children: ESCodeProcessChildProcess[];
}

export interface ESCodeAgentStorageStartupSnapshot {
  generation: number;
  state: ESCodeStorageStartupState | null;
}

export interface IESCodeAgentService {
  /** 控制面不需要账号或模型，且不发送普通协议请求。 */
  prepareStorage(params: ESCodeAgentWorkspaceTarget): Promise<void>;
  getStorageStartupState(
    params: ESCodeAgentWorkspaceTarget,
  ): Promise<ESCodeAgentStorageStartupSnapshot | null>;
  onDynamicStorageStartupState(
    params: ESCodeAgentWorkspaceTarget,
  ): Event<ESCodeAgentStorageStartupSnapshot>;
  initialize(params: ESCodeAgentWorkspaceTarget): Promise<ESCodeAgentInitializeResult>;
  /**
   * 同步 App 全局运行时偏好到所有已活动 workspace；不得为此启动空闲 Agent。
   */
  syncAppRuntimePreferences(preferences: ESCodeAgentAppRuntimePreferences): Promise<void>;
  getWorkspaceRuntimeIdentity(
    params: ESCodeAgentWorkspaceTarget,
  ): Promise<ESCodeAgentWorkspaceRuntimeIdentity>;
  createSession(params: ESCodeAgentCreateSessionParams): Promise<ESCodeSessionStateSnapshot>;
  resumeSession(params: ESCodeAgentResumeSessionParams): Promise<ESCodeSessionStateSnapshot>;
  listSessions(params: ESCodeAgentListSessionsParams): Promise<ESCodeSessionInfo[]>;
  listSessionSubagents(
    params: ESCodeAgentListSessionSubagentsParams,
  ): Promise<ESCodeSessionSubagentsResult>;
  getAppUsageStats(params: ESCodeAgentAppUsageParams): Promise<AppUsageSnapshot>;
  getTaskTokenUsage(params: ESCodeAgentTaskTokenUsageParams): Promise<ESCodeTaskTokenUsageResult>;
  readSession(params: ESCodeAgentReadSessionParams): Promise<ESCodeSessionStateSnapshot>;
  readSessionMessages(
    params: ESCodeAgentReadSessionMessagesParams,
  ): Promise<ESCodeMessageWithParts[]>;
  readSessionDebug(params: ESCodeAgentSessionTarget): Promise<SessionDebugSnapshot>;
  readSessionEvents(params: ESCodeAgentReadSessionEventsParams): Promise<ESCodeSessionEvent[]>;
  readWorkspacePresentation(
    params: ESCodeAgentReadWorkspacePresentationParams,
  ): Promise<ESCodeWorkspacePresentation>;
  /** 无 task/session 的 Settings 预信任；Agent 会重新发现并校验 canonical snapshot。 */
  grantWorkspaceHookTrust(
    params: ESCodeAgentGrantWorkspaceHookTrustParams,
  ): Promise<ESCodeWorkspaceHookTrustGrantResult>;
  listMcpServerStatuses(params: ESCodeAgentListMcpServerStatusesParams): Promise<ESCodeMcpListResult>;
  listPlugins(params: ESCodeAgentPluginViewParams): Promise<ESCodePluginsListResult>;
  /**
   * Plugin 对话引用 catalog：session-scoped 只读投影。
   * 走 workspace 级 agent client（session 记录只存在于该进程），不走独立插件管理进程。
   */
  getPluginReferenceCatalog(
    params: ESCodeAgentPluginReferenceCatalogParams,
  ): Promise<ESCodePluginsReferenceCatalogResult>;
  /** Composer Skill 引用 catalog；带 sessionId 时读取该 runtime 的冻结快照。 */
  getSkillReferenceCatalog(
    params: ESCodeAgentSkillReferenceCatalogParams,
  ): Promise<ESCodeSkillsReferenceCatalogResult>;
  // 已保存工作流的 GUI 中枢：workspace 级、无会话，每次调用现扫 `<cwd>/.escode/workflows/`。
  // 全局档传 `scope: "global"`：带 workspace 就用它当载体，不带则由 services 层自选本机载体运行时。
  listSavedWorkflows(params: ESCodeAgentListSavedWorkflowsParams): Promise<ESCodeWorkflowsListResult>;
  getSavedWorkflow(params: ESCodeAgentGetSavedWorkflowParams): Promise<ESCodeWorkflowsGetResult>;
  updateSavedWorkflowMeta(
    params: ESCodeAgentUpdateSavedWorkflowMetaParams,
  ): Promise<ESCodeWorkflowsUpdateMetaResult>;
  deleteSavedWorkflow(
    params: ESCodeAgentDeleteSavedWorkflowParams,
  ): Promise<ESCodeWorkflowsDeleteResult>;
  listSavedWorkflowRuns(
    params: ESCodeAgentListSavedWorkflowRunsParams,
  ): Promise<ESCodeWorkflowsRunsResult>;
  // 在项目档 / 全局档之间移动同名文件：
  // `workspace` 是载体（移到项目传目标项目、移到全局传源项目），`to` 是落点档；不覆盖已存在的目标。
  moveSavedWorkflow(params: ESCodeAgentMoveSavedWorkflowParams): Promise<ESCodeWorkflowsMoveResult>;
  resolveSuggestedPluginReference(
    params: ESCodeAgentResolveSuggestedPluginReferenceParams,
  ): Promise<import("@escode/shared").ESCodePluginsResolveSuggestedReferenceResult>;
  /** 推荐项 Plugin 首次本地检查缺失后的 operation-scoped 刷新进度。 */
  onDynamicPluginOperationProgress(
    operationId: string,
  ): Event<ESCodePluginOperationProgressNotification>;
  getPluginsOverview(params: ESCodeAgentPluginViewParams): Promise<ESCodePluginsOverviewResult>;
  /**
   * 资源管理器：枚举本 Host 内全部本地 Agent 进程（含 plugin / mcp-status 泳道），
   * 并向每个存活 runtime 请求 `process/childProcesses`；单个 runtime 失败只让它的 children 为空。
   */
  collectLocalRuntimeChildProcesses(
    signal?: AbortSignal,
  ): Promise<ESCodeAgentLocalRuntimeChildProcesses[]>;
  addPluginMarketplace(
    params: ESCodeAgentAddPluginMarketplaceParams,
  ): Promise<ESCodePluginsMarketplaceMutationResult>;
  removePluginMarketplace(
    params: ESCodeAgentRemovePluginMarketplaceParams,
  ): Promise<ESCodePluginsMarketplaceMutationResult>;
  updatePluginMarketplace(
    params: ESCodeAgentUpdatePluginMarketplaceParams,
  ): Promise<ESCodePluginsMarketplaceMutationResult>;
  installPlugin(params: ESCodeAgentInstallPluginParams): Promise<ESCodePluginsInstallResult>;
  cancelPluginOperation(
    params: ESCodeAgentCancelPluginOperationParams,
  ): Promise<ESCodePluginsCancelOperationResult>;
  uninstallPlugin(params: ESCodeAgentUninstallPluginParams): Promise<ESCodePluginsUninstallResult>;
  updatePlugin(params: ESCodeAgentUpdatePluginParams): Promise<ESCodePluginsInstallResult>;
  restoreBuiltinPlugin(
    params: ESCodeAgentRestoreBuiltinPluginParams,
  ): Promise<ESCodePluginsRestoreBuiltinResult>;
  configurePlugin(params: ESCodeAgentConfigurePluginParams): Promise<ESCodePluginsConfigureResult>;
  resetPluginConfig(
    params: ESCodeAgentResetPluginConfigParams,
  ): Promise<ESCodePluginsConfigureResult>;
  validatePlugin(params: ESCodeAgentValidatePluginParams): Promise<ESCodePluginsValidateResult>;
  describePlugin(params: ESCodeAgentDescribePluginParams): Promise<ESCodePluginsDescribeResult>;
  setPluginEnabled(params: ESCodeAgentSetPluginEnabledParams): Promise<ESCodePluginsSetEnabledResult>;
  // ---- 定时任务(automation)管理 ----
  listAutomations(params: ESCodeAgentWorkspaceTarget): Promise<ESCodeAutomation[]>;
  listAllAutomations(): Promise<ESCodeAutomation[]>;
  createAutomation(params: ESCodeAgentCreateAutomationParams): Promise<ESCodeAutomation>;
  updateAutomation(params: ESCodeAgentUpdateAutomationParams): Promise<ESCodeAutomation | null>;
  deleteAutomation(params: ESCodeAgentAutomationIdParams): Promise<void>;
  setAutomationEnabled(params: ESCodeAgentSetAutomationEnabledParams): Promise<void>;
  restartAutomation(params: ESCodeAgentAutomationIdParams): Promise<void>;
  runAutomationNow(params: ESCodeAgentAutomationIdParams): Promise<ESCodeAgentRunAutomationNowResult>;
  listAutomationRuns(params: ESCodeAgentAutomationIdParams): Promise<ESCodeAutomationRun[]>;
  deleteAutomationRun(params: ESCodeAgentDeleteAutomationRunParams): Promise<void>;
  generateWorkspaceText(
    params: ESCodeAgentGenerateWorkspaceTextParams,
  ): Promise<ESCodeWorkspaceGenerateTextResult>;
  testModelConnectivity(
    params: ESCodeAgentTestModelConnectivityParams,
  ): Promise<ESCodeProviderTestModelConnectivityResult>;
  /**
   * @deprecated：send 主路径已收敛 v4 sendText 命令。仅剩两个消费点——
   * adapter 带附件输入回退（待附件命令面落地后移除）与 escodeSessionService
   * pass-through；新代码禁止回用。
   */
  sendPrompt(params: ESCodeAgentSendPromptParams): Promise<ESCodeSessionSendResult>;
  compactSession(params: ESCodeAgentCompactParams): Promise<ESCodeSessionCompactResult>;
  goalSession(params: ESCodeAgentGoalParams): Promise<ESCodeSessionGoalResult>;
  closeSession(
    params: ESCodeAgentSessionTarget & { expectedPersistence?: "deferred" | "immediate" },
  ): Promise<boolean>;
  setModel(params: ESCodeAgentSetModelParams): Promise<ESCodeSessionStateSnapshot>;
  setThoughtLevel(params: ESCodeAgentSetThoughtLevelParams): Promise<ESCodeSessionStateSnapshot>;
  setMode(params: ESCodeAgentSetModeParams): Promise<ESCodeSessionStateSnapshot>;
  respondSessionRuntimePreferences(
    params: ESCodeAgentRespondSessionRuntimePreferencesParams,
  ): Promise<void>;
  onDynamicSessionRuntimePreferencesRequest(): Event<ESCodeAgentSessionRuntimePreferencesRequest>;
  /**
   * CLI 进程级资源样本，带 services 打的 lane 标签（CLI 自己不知道 lane）。
   * 使用 dynamic event 避免 RPC 服务在无人订阅时缓冲周期事件；
   * 该事件不属于 session/conversation continuous 或 replayable 状态。
   */
  onDynamicProcessResourceSample(): Event<AgentLaneResourceSample>;
  /** MCP 进程生命周期与低频内存事件，仅供可信 Host relay 上报 ARMS。 */
  onDynamicMcpTelemetry(): Event<ESCodeMcpTelemetryEvent>;
  /** MCP 进程树资源事实，只供可信 Host 汇总上报。 */
  onDynamicMcpResourceSamples(): Event<ESCodeMcpResourceSample[]>;
  /** Bash 完成事实，仅可信 Host 资源旁路订阅。 */
  onDynamicToolExecResource(): Event<ESCodeToolExecResource>;
  /**
   * @deprecated 旧协议订阅面（session/subscribe + session/event + state.updated）。
   * task-index syncer 已迁 v4 sessions-index/workspace-config 帧；
   * 仅剩 escodeTaskServiceAdapter.onDynamicTaskEvent（replayable 读路径）消费。
   * 写路径已收敛 v4 命令面；本订阅是读路径投影源。
   */
  onDynamicSessionEvent(params: ESCodeAgentSessionSubscribeParams): Event<ESCodeAgentServiceEvent>;
  // ── v4 conversation 通道（竖切）──
  /** RPC attachment 建立后先读取 host 可信 hello。 */
  helloConversationV4(): Promise<HelloMessage>;
  /** hello 校验后回送 clientHello；metadata 不能覆盖 connection mode/profile。 */
  initializeConversationV4(clientHello: ClientHello): Promise<void>;
  /** 仅供 trusted host relay/facade；terminal RPC caller 必须被 connection scope 拒绝。 */
  setConnectionFlowStateV4(params: ESCodeAgentConnectionFlowParams): Promise<void>;
  subscribeConversationV4(
    params: ESCodeAgentConversationSubscribeParams,
  ): Promise<V4ConversationSubscribeResult>;
  resyncConversationV4(
    params: ESCodeAgentConversationResyncParams,
  ): Promise<V4ConversationResyncResult>;
  unsubscribeConversationV4(params: ESCodeAgentConversationUnsubscribeParams): Promise<void>;
  /** rows/range 行分页 query（loadOlder 游标向上补历史）。 */
  conversationRowsRangeV4(
    params: ESCodeAgentConversationRowsRangeParams,
  ): Promise<V4ConversationRowsRangeResult>;
  conversationPlansV4(
    params: ESCodeAgentConversationPlansParams,
  ): Promise<V4ConversationPlansResult>;
  /** workflow run 事件日志分页；与 plans 同族（只读、无状态、超时重发安全）。 */
  conversationWorkflowRunEventsV4(
    params: ESCodeAgentConversationWorkflowRunEventsParams,
  ): Promise<V4ConversationWorkflowRunEventsResult>;
  /** workflow run 枚举；journal-backed 的重启后发现面。 */
  conversationWorkflowRunsV4(
    params: ESCodeAgentConversationWorkflowRunsParams,
  ): Promise<V4ConversationWorkflowRunsResult>;
  /** workflow run 的用户面产物清单；与 plans 同族（只读、无状态、超时重发安全）。 */
  conversationWorkflowRunArtifactsV4(
    params: ESCodeAgentConversationWorkflowRunArtifactsParams,
  ): Promise<V4ConversationWorkflowRunArtifactsResult>;
  /** 预置看板的条目分页；hook 以 itemCount 变化为信号增量拉取。 */
  conversationWorkflowRunArtifactDataV4(
    params: ESCodeAgentConversationWorkflowRunArtifactDataParams,
  ): Promise<V4ConversationWorkflowRunArtifactDataResult>;
  /** 内容产物的字节，一次一块；授权在 CLI 侧（journal 行才是取字节的依据）。 */
  conversationWorkflowRunArtifactReadV4(
    params: ESCodeAgentConversationWorkflowRunArtifactReadParams,
  ): Promise<V4ConversationWorkflowRunArtifactReadResult>;
  /** dwf 工作区 transcript 的清单。 */
  conversationWorkflowRunWorkspaceV4(
    params: ESCodeAgentConversationWorkflowRunWorkspaceParams,
  ): Promise<V4ConversationWorkflowRunWorkspaceResult>;
  /** 一个工作区节点的有界正文。 */
  conversationWorkflowRunNodeResultV4(
    params: ESCodeAgentConversationWorkflowRunNodeResultParams,
  ): Promise<V4ConversationWorkflowRunNodeResultResult>;
  backgroundBashOutputV4(
    params: ESCodeAgentBackgroundBashOutputParams,
  ): Promise<BackgroundBashOutputResult>;
  conversationFileChangesV4(
    params: ESCodeAgentConversationFileChangesParams,
  ): Promise<V4ConversationFileChangesResult>;
  conversationFileRewindPreviewV4(
    params: ESCodeAgentConversationFileRewindPreviewParams,
  ): Promise<V4ConversationFileRewindPreviewResult>;
  sendConversationCommandV4(params: ESCodeAgentConversationCommandParams): Promise<CommandAck>;
  queryConversationCommandsV4(params: ESCodeAgentCommandsQueryParams): Promise<CommandsQueryResult>;
  attachmentBeginV4(params: ESCodeAgentAttachmentBeginParams): Promise<V4AttachmentBeginResult>;
  attachmentChunkV4(params: ESCodeAgentAttachmentChunkParams): Promise<V4AttachmentChunkResult>;
  attachmentCommitV4(params: ESCodeAgentAttachmentTerminalParams): Promise<V4AttachmentCommitResult>;
  attachmentAbortV4(params: ESCodeAgentAttachmentTerminalParams): Promise<void>;
  /** Desktop local 已发送视频 source query；远端与 Web 返回 chunked。 */
  attachmentPreviewSourceV4(
    params: ESCodeAgentAttachmentPreviewSourceParams,
  ): Promise<V4AttachmentPreviewSourceResult>;
  /** 已发送 image/video 只读分块查询；connection scope 注入可信 workspace 连接。 */
  attachmentReadV4(params: ESCodeAgentAttachmentReadParams): Promise<V4AttachmentReadResult>;
  /** Share 读取 userInput 附件，允许 text/plain 等非媒体类型。 */
  conversationAttachmentReadV4(
    params: ESCodeAgentConversationAttachmentReadParams,
  ): Promise<V4ConversationAttachmentReadResult>;
  /** Share 选择阶段只读 userInput 附件元数据，不读取完整内容。 */
  conversationAttachmentStatV4(
    params: ESCodeAgentConversationAttachmentStatParams,
  ): Promise<V4ConversationAttachmentStatResult>;
  /** workspace 级下行帧流（v4/conversation/frame），renderer 侧按 topic 自行路由。 */
  onDynamicConversationFrame(
    params: ESCodeAgentWorkspaceTarget,
  ): Event<ConversationTopicWireCandidate>;
  /** workspace 级 live telemetry 事实；connection facade 仅向可信 desktop-continuous 下游暴露。 */
  onDynamicLocalTtftFacts(
    params: ESCodeAgentWorkspaceTarget,
  ): Event<import("@escode/shared").LocalTtftFacts>;
  onDynamicConversationTelemetryFact(
    params: ESCodeAgentWorkspaceTarget,
  ): Event<ConversationTelemetryFact>;
  /** 当前窗口全部本地 live task 的 CUA 权限观察；历史、远程与 replayable 不在此事件面。 */
  onDynamicCuaPermissionObservation(): Event<ESCodeAgentCuaPermissionObservation>;
  // ── sessions-index 通道（列表活性）──
  subscribeSessionsIndexV4(
    params: ESCodeAgentSessionsIndexSubscribeParams,
  ): Promise<V4SessionsIndexSubscribeResult>;
  resyncSessionsIndexV4(
    params: ESCodeAgentConversationResyncParams,
  ): Promise<V4ConversationResyncResult>;
  unsubscribeSessionsIndexV4(params: ESCodeAgentConversationUnsubscribeParams): Promise<void>;
  /** workspace 级 sessions-index 下行帧流（与 conversation 同一通知，按 topic 前缀分流）。 */
  onDynamicSessionsIndexFrame(
    params: ESCodeAgentWorkspaceTarget,
  ): Event<SessionsIndexTopicWireCandidate>;
  // ── workspace-config 通道（配置目录活性；task-index syncer 消费）──
  subscribeWorkspaceConfigV4(
    params: ESCodeAgentWorkspaceConfigSubscribeParams,
  ): Promise<V4WorkspaceConfigSubscribeResult>;
  resyncWorkspaceConfigV4(
    params: ESCodeAgentConversationResyncParams,
  ): Promise<V4ConversationResyncResult>;
  unsubscribeWorkspaceConfigV4(params: ESCodeAgentConversationUnsubscribeParams): Promise<void>;
  /** workspace 级 workspace-config 下行帧流（与 conversation 同一通知，按 topic 前缀分流）。 */
  onDynamicWorkspaceConfigFrame(
    params: ESCodeAgentWorkspaceTarget,
  ): Event<WorkspaceConfigTopicWireCandidate>;
  /**
   * （CLI 重连重订）：agent 进程换代通知（超时回收/崩溃后重新拉起）。
   * v4 订阅活在 CLI 进程内存，进程换代即失效；订阅方（task-index syncer 等）
   * 收到后必须对该 workspaceKey 重发 subscribe，否则帧流静默中断。
   */
  onAgentRuntimeRestarted(listener: (event: { workspaceKey: string }) => void): IDisposable;
  /**
   * Agent client 在 service 内完成登记后发布 available，当前 client 关闭后发布 unavailable。
   * 这是被动 observer attach/detach 的唯一生命周期信号，不表达用户使用租约。
   */
  onAgentRuntimeLifecycle?: (
    listener: (event: ESCodeAgentRuntimeLifecycleEvent) => void,
  ) => IDisposable;
  /** 当前 desktop-local CUA turn 是否仍在执行，用于 Helper recovery 避免中途回收 Agent。 */
  hasActiveCuaOperationTurn(): boolean;
  disposeWorkspace(params: ESCodeAgentWorkspaceTarget): Promise<void>;
  disposeAll(): void;
}

export const IESCodeAgentService = createServiceDescriptor<IESCodeAgentService>(
  ServiceChannels.ESCodeAgent,
);
