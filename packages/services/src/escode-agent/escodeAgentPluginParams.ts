import type {
  ModelSelection,
  ESCodeAgentMcpServer,
  ESCodeAutomationScheduleRule,
  ESCodeMcpListMode,
} from "@escode/shared";
import type { McpAppsAppToolCallResult, McpAppsAppToolDescriptor } from "@escode/shared/mcp-apps";

export interface ESCodeAgentWorkspaceTarget {
  workspacePath: string;
  workspaceIdentity?: string;
  /** 远程 workspace 的运行时会话身份；只用于隔离/路由，不能替代 workspacePath。 */
  remoteSessionId?: string;
}

export interface ESCodeAgentPluginViewParams extends ESCodeAgentWorkspaceTarget {
  configScope?: "user" | "workspace";
}

export interface ESCodeAgentListMcpServerStatusesParams extends ESCodeAgentWorkspaceTarget {
  mcpServers?: ESCodeAgentMcpServer[];
  mode?: ESCodeMcpListMode;
}

export interface ESCodeAgentAddPluginMarketplaceParams extends ESCodeAgentWorkspaceTarget {
  dryRun?: boolean;
  operationId?: string;
  source: string;
}

export interface ESCodeAgentRemovePluginMarketplaceParams extends ESCodeAgentWorkspaceTarget {
  marketplace: string;
}

export interface ESCodeAgentUpdatePluginMarketplaceParams extends ESCodeAgentWorkspaceTarget {
  marketplace?: string;
  operationId?: string;
}

export interface ESCodeAgentInstallPluginParams extends ESCodeAgentWorkspaceTarget {
  dryRun?: boolean;
  marketplace: string;
  operationId?: string;
  pluginName: string;
  scope?: "user" | "workspace";
}

export interface ESCodeAgentCancelPluginOperationParams {
  operationId: string;
}

export interface ESCodeAgentUninstallPluginParams extends ESCodeAgentWorkspaceTarget {
  marketplace?: string;
  pluginId?: string;
  pluginName?: string;
  removeCache?: boolean;
}

export interface ESCodeAgentUpdatePluginParams extends ESCodeAgentWorkspaceTarget {
  pluginId?: string;
  marketplace?: string;
}

export interface ESCodeAgentRestoreBuiltinPluginParams extends ESCodeAgentWorkspaceTarget {
  pluginId: string;
}

export interface ESCodeAgentConfigurePluginParams extends ESCodeAgentWorkspaceTarget {
  clearOptionKeys?: string[];
  dryRun?: boolean;
  options: Record<string, unknown>;
  pluginId: string;
  scope?: "user" | "workspace";
}

export interface ESCodeAgentResetPluginConfigParams extends ESCodeAgentWorkspaceTarget {
  pluginId: string;
  scope?: "user" | "workspace";
}

export interface ESCodeAgentValidatePluginParams extends ESCodeAgentWorkspaceTarget {
  marketplace?: string;
  pluginName?: string;
  source?: string;
}

export interface ESCodeAgentDescribePluginParams extends ESCodeAgentWorkspaceTarget {
  marketplace: string;
  pluginName: string;
}

export interface ESCodeAgentSetPluginEnabledParams extends ESCodeAgentWorkspaceTarget {
  enabled: boolean;
  operationId?: string;
  pluginId: string;
  scope?: "user" | "workspace";
}

// Plugin 对话引用 catalog：
// 带 sessionId → session-owned 冻结 catalog（必须路由到持有该 session 的 workspace client）；
// 不带 → workspace 当前 catalog（新建草稿 Picker）。
export interface ESCodeAgentPluginReferenceCatalogParams extends ESCodeAgentWorkspaceTarget {
  sessionId?: string;
}

// Composer Skill catalog：与 Plugin 引用相同，以 sessionId 区分 workspace 当前目录和
// resident Session runtime 快照；不参与 Settings 管理目录。
export interface ESCodeAgentSkillReferenceCatalogParams extends ESCodeAgentWorkspaceTarget {
  sessionId?: string;
}

/** 插件 UI：UI 发起的 `ui://` 资源读取。 */
export interface ESCodeAgentReadMcpResourceParams extends ESCodeAgentWorkspaceTarget {
  instance: import("@escode/shared/mcp-apps").McpAppInstance;
  sessionId: string;
  pluginId: string;
  serverName: string;
  uri: string;
}

/** 插件 UI 页面发起的 `resources/read`；参数与 readMcpResource 同形，结果受 8 MiB 与 mimeType 白名单约束。 */
export type ESCodeAgentReadMcpResourceForUiParams = ESCodeAgentReadMcpResourceParams;

/** 插件 UI：UI 回调本插件工具；不进审批流，归属由 agent 侧 catalog fail closed。 */
export interface ESCodeAgentCallMcpToolForUiParams extends ESCodeAgentWorkspaceTarget {
  instance: import("@escode/shared/mcp-apps").McpAppInstance;
  sessionId: string;
  pluginId: string;
  serverName: string;
  toolName: string;
  arguments?: Record<string, unknown>;
  /** 宿主生成的调用 id，配合 cancelMcpToolCallForUi。 */
  callId: string;
}
/** 插件 UI 页面发起的 resources/list、resources/templates/list、subscribe、unsubscribe。 */
export interface ESCodeAgentListMcpResourcesForUiParams extends ESCodeAgentWorkspaceTarget {
  instance: import("@escode/shared/mcp-apps").McpAppInstance;
  sessionId: string;
  pluginId: string;
  serverName: string;
  cursor?: string;
}
/** App-Provided Tools：实例身份 = 会话 + 沙箱作用域 + 代际。 */
export interface ESCodeAgentAppToolInstanceForUiParams extends ESCodeAgentWorkspaceTarget {
  instance: import("@escode/shared/mcp-apps").McpAppInstance;
  sessionId: string;
  pluginId: string;
  serverName: string;
  scopeId: string;
  generation: number;
}
export interface ESCodeAgentRegisterAppToolsForUiParams extends ESCodeAgentAppToolInstanceForUiParams {
  tools: McpAppsAppToolDescriptor[];
}
export interface ESCodeAgentAppToolCallForUiParams extends ESCodeAgentAppToolInstanceForUiParams {
  callId: string;
}
export interface ESCodeAgentResolveAppToolCallForUiParams extends ESCodeAgentAppToolCallForUiParams {
  result?: McpAppsAppToolCallResult;
  error?: { message: string };
}
export interface ESCodeAgentMcpResourceSubscriptionForUiParams extends ESCodeAgentWorkspaceTarget {
  instance: import("@escode/shared/mcp-apps").McpAppInstance;
  sessionId: string;
  pluginId: string;
  serverName: string;
  scopeId: string;
  generation: number;
  uri: string;
}
export interface ESCodeAgentCancelMcpToolCallForUiParams extends ESCodeAgentWorkspaceTarget {
  instance: import("@escode/shared/mcp-apps").McpAppInstance;
  sessionId: string;
  pluginId: string;
  serverName: string;
  callId: string;
}
export interface ESCodeAgentResolveSuggestedPluginReferenceParams extends ESCodeAgentWorkspaceTarget {
  stableId: string;
  operationId: string;
  clientMode: "desktop-continuous" | "web-remote-replayable";
  deliveryKind: "desktop-continuous" | "web-remote-replayable";
}

// ---- 定时任务(automation)管理参数 ----

export interface ESCodeAgentCreateAutomationParams extends ESCodeAgentWorkspaceTarget {
  title: string;
  cronExpr: string;
  relativeDelayMinutes?: number;
  prompt: string;
  modelSelection?: ModelSelection;
  mode?: string;
  recurring?: boolean;
  maxRuns?: number;
  endAt?: number;
  scheduleRule?: ESCodeAutomationScheduleRule;
}

export interface ESCodeAgentUpdateAutomationParams extends ESCodeAgentWorkspaceTarget {
  automationId: string;
  title?: string;
  cronExpr?: string;
  prompt?: string;
  modelSelection?: ModelSelection | null;
  mode?: string | null;
  recurring?: boolean;
  maxRuns?: number | null;
  endAt?: number | null;
  scheduleRule?: ESCodeAutomationScheduleRule | null;
  scheduleEditedByUser?: boolean;
}

export interface ESCodeAgentAutomationIdParams extends ESCodeAgentWorkspaceTarget {
  automationId: string;
}

export interface ESCodeAgentSetAutomationEnabledParams extends ESCodeAgentWorkspaceTarget {
  automationId: string;
  enabled: boolean;
}

export interface ESCodeAgentDeleteAutomationRunParams extends ESCodeAgentWorkspaceTarget {
  runId: string;
}
