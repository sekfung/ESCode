import { ServiceChannels } from "@escode/shared";
import type {
  TraceId,
  ESCodeAgentMcpServer,
  ESCodeDeliveryKind,
  ESCodeMessageWithParts,
  ModelSelection,
  ESCodePermissionRequestParams,
  ESCodeUserInputRequestParams,
  ESCodeUserInputResponse,
  ESCodeSessionInfo,
  ESCodeSessionImportHistory,
  ESCodeSessionEvent,
  ESCodeSessionMode,
  ESCodeSessionPersistence,
  ESCodeSessionStateSnapshot,
  ESCodeStateUpdatedNotification,
  ESCodeWorkspacePresentation,
} from "@escode/shared";
import { createServiceDescriptor } from "#src/descriptors.js";

export interface ESCodeSessionWorkspaceTarget {
  workspacePath: string;
  workspaceIdentity?: string;
  remoteSessionId?: string;
}

export type ESCodeSessionReadWorkspacePresentationParams = ESCodeSessionWorkspaceTarget;

export interface ESCodeTaskTarget extends ESCodeSessionWorkspaceTarget {
  sessionId: string;
}

export interface ESCodeSessionCreateParams extends ESCodeSessionWorkspaceTarget {
  /** 仅导入事务使用的预分配 ID；普通新会话继续由 Agent 分配。 */
  sessionId?: string;
  sessionTraceId?: TraceId;
  parentSessionId?: string;
  mode?: ESCodeSessionMode;
  model?: ModelSelection;
  persistence?: ESCodeSessionPersistence;
  thoughtLevel?: string;
  mcpServers?: ESCodeAgentMcpServer[];
  importedHistory?: ESCodeSessionImportHistory;
}

export interface ESCodeSessionResumeParams extends ESCodeTaskTarget {
  model?: ModelSelection;
  thoughtLevel?: string;
  mcpServers?: ESCodeAgentMcpServer[];
  /**
   * 默认广播 resume 得到的历史快照，并让 shadow 订阅请求初始 snapshot。
   * 续聊发送前的 runtime 预恢复会关闭它，避免旧终态快照覆盖本地已开始的新输入运行态。
   */
  broadcastSnapshot?: boolean;
}

export interface ESCodeSessionListParams extends ESCodeSessionWorkspaceTarget {
  includeArchived?: boolean;
  limit?: number;
}

export interface ESCodeSessionReadParams extends ESCodeTaskTarget {
  deliveryKind?: ESCodeDeliveryKind;
  messageLimit?: number;
  afterSeq?: number;
}

export interface ESCodeSessionMessagesParams extends ESCodeTaskTarget {
  afterMessageId?: string;
  limit?: number;
}

export interface ESCodeSessionEventsParams extends ESCodeTaskTarget {
  afterSeq?: number;
  limit?: number;
}

export interface ESCodeSessionSetModelParams extends ESCodeTaskTarget {
  model: ModelSelection;
  expectedRevision?: number;
  persistAsWorkspaceLastUsed?: boolean;
}

export interface ESCodeSessionSetThoughtLevelParams extends ESCodeTaskTarget {
  thoughtLevel?: string;
  expectedRevision?: number;
  persistAsWorkspaceLastUsed?: boolean;
}

export interface ESCodeSessionSetModeParams extends ESCodeTaskTarget {
  mode: ESCodeSessionMode;
  expectedRevision?: number;
}

export interface ESCodeSessionSubscribeParams extends ESCodeTaskTarget {
  deliveryKind: ESCodeDeliveryKind;
  afterSeq?: number;
  includeSnapshot?: boolean;
  eventCoalescing?: {
    mode: "background-summary";
    intervalMs?: number;
  };
}

export type ESCodeSessionServiceEvent =
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

export interface ESCodeSessionInitializeResult {
  available: boolean;
  workspaceKey: string;
  protocolName?: string;
  protocolVersion?: number;
  transportKind?: "stdio" | "websocket";
  reason?: string;
  reasonCode?: "provider_not_ready";
}

export interface ESCodeSessionWorkspaceRuntimeIdentity {
  generation: number;
  identity: string;
  processId?: number;
  workspaceKey: string;
}

export interface IESCodeSessionService {
  initializeWorkspace(params: ESCodeSessionWorkspaceTarget): Promise<ESCodeSessionInitializeResult>;
  getWorkspaceRuntimeIdentity(
    params: ESCodeSessionWorkspaceTarget,
  ): Promise<ESCodeSessionWorkspaceRuntimeIdentity>;
  readWorkspacePresentation(
    params: ESCodeSessionReadWorkspacePresentationParams,
  ): Promise<ESCodeWorkspacePresentation>;
  createSession(params: ESCodeSessionCreateParams): Promise<ESCodeSessionStateSnapshot>;
  resumeSession(params: ESCodeSessionResumeParams): Promise<ESCodeSessionStateSnapshot>;
  listSessions(params: ESCodeSessionListParams): Promise<ESCodeSessionInfo[]>;
  readSession(params: ESCodeSessionReadParams): Promise<ESCodeSessionStateSnapshot>;
  readSessionMessages(params: ESCodeSessionMessagesParams): Promise<ESCodeMessageWithParts[]>;
  readSessionEvents(params: ESCodeSessionEventsParams): Promise<ESCodeSessionEvent[]>;
  promoteDeferredDraftSession(params: ESCodeTaskTarget): Promise<void>;
  closeSession(params: ESCodeTaskTarget): Promise<void>;
  closeDeferredDraftSession(params: ESCodeTaskTarget): Promise<boolean>;
  setModel(params: ESCodeSessionSetModelParams): Promise<ESCodeSessionStateSnapshot>;
  setThoughtLevel(params: ESCodeSessionSetThoughtLevelParams): Promise<ESCodeSessionStateSnapshot>;
  setMode(params: ESCodeSessionSetModeParams): Promise<ESCodeSessionStateSnapshot>;
  // renderer 订阅面走 agentService 的 conversation/sessions-index 帧通道。
}

export const IESCodeSessionService = createServiceDescriptor<IESCodeSessionService>(
  ServiceChannels.ESCodeSession,
);
