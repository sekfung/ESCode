// 平台能力面收敛：设置页「插件管理」的薄服务接口。
//
// 背景：pluginManagementStore / usePluginUninstall 过去直接注入 IESCodeAgentService，
// UI 层因此散布 13 个 plugins/* 旧协议词的消费点。收敛为独立薄 service 后，UI 只依赖
// 本接口；plugins/* 词表的 host 侧消费点收拢到 pluginManagementService 一处（插件的
// 事实源在 escode-cli 进程，服务实现仍经 agent 协议往返——plugins 词表的收口归属
// 插件能力面自身的协议演进，不在会话 v4 词表范围内）。
// 注意与既有 IPluginsService（已 retired 的 marketplace pluginStore 通道）区分：
// 那套接口按 pluginName+marketplace 寻址且方法语义过时，不复用避免签名冲突。
import type { Event } from "@escode/rpc";
import type {
  ESCodePluginOperationProgressNotification,
  ESCodePluginsConfigureResult,
  ESCodePluginsCancelOperationResult,
  ESCodePluginsDescribeResult,
  ESCodePluginsInstallResult,
  ESCodePluginsListResult,
  ESCodePluginsMarketplaceMutationResult,
  ESCodePluginsOverviewResult,
  ESCodePluginsReferenceCatalogResult,
  ESCodePluginsRestoreBuiltinResult,
  ESCodePluginsSetEnabledResult,
  ESCodePluginsUninstallResult,
  ESCodePluginsValidateResult,
} from "@escode/shared";
import { ServiceChannels } from "@escode/shared";
import { createServiceDescriptor } from "../descriptors.js";
import type {
  ESCodeAgentAddPluginMarketplaceParams,
  ESCodeAgentConfigurePluginParams,
  ESCodeAgentCancelPluginOperationParams,
  ESCodeAgentDescribePluginParams,
  ESCodeAgentInstallPluginParams,
  ESCodeAgentPluginReferenceCatalogParams,
  ESCodeAgentResolveSuggestedPluginReferenceParams,
  ESCodeAgentResetPluginConfigParams,
  ESCodeAgentPluginViewParams,
  ESCodeAgentRemovePluginMarketplaceParams,
  ESCodeAgentRestoreBuiltinPluginParams,
  ESCodeAgentSetPluginEnabledParams,
  ESCodeAgentUninstallPluginParams,
  ESCodeAgentUpdatePluginMarketplaceParams,
  ESCodeAgentUpdatePluginParams,
  ESCodeAgentValidatePluginParams,
} from "../escode-agent/escodeAgentPluginParams.js";

export interface IPluginManagementService {
  listPlugins(params: ESCodeAgentPluginViewParams): Promise<ESCodePluginsListResult>;
  /**
   * Plugin 对话引用 catalog：
   * 带 sessionId → session-owned 冻结 catalog；不带 → workspace 当前 catalog。
   * 实现路由到 workspace 级 agent client，不走插件管理独立进程。
   */
  getPluginReferenceCatalog(
    params: ESCodeAgentPluginReferenceCatalogParams,
  ): Promise<ESCodePluginsReferenceCatalogResult>;
  resolveSuggestedPluginReference(
    params: ESCodeAgentResolveSuggestedPluginReferenceParams,
  ): Promise<import("@escode/shared").ESCodePluginsResolveSuggestedReferenceResult>;
  onDynamicPluginOperationProgress(
    operationId: string,
  ): Event<ESCodePluginOperationProgressNotification>;
  getPluginsOverview(params: ESCodeAgentPluginViewParams): Promise<ESCodePluginsOverviewResult>;
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
}

export const IPluginManagementService = createServiceDescriptor<IPluginManagementService>(
  ServiceChannels.PluginManagement,
);
