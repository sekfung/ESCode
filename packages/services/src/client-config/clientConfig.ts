import type { ModelSelectionView } from "@escode/provider";
import {
  ServiceChannels,
  type ClientConfigReadOptions,
  type ClientConfigSnapshot,
  type DynamicWorkflowClientConfig,
} from "@escode/shared";
import { createServiceDescriptor } from "../descriptors.js";

/** 闲时任务客户端配置（原挂在 coding-plan-subscription 通道；套餐族删除后随 client-config 通道下发）。 */
export interface OffPeakClientConfig {
  readonly enabled: boolean;
  readonly modelSelectionView: ModelSelectionView;
  /** mock 演示强制通道：真实路径不下发，UI 使用 Host 的 Account support。 */
  readonly codingPlanActive?: boolean;
}

/** 窗口级公开配置读取。业务模块只消费自己的字段，不拥有第二份请求缓存。 */
export interface IClientConfigService {
  getSnapshot(options?: ClientConfigReadOptions): Promise<ClientConfigSnapshot>;
  /** 闲时任务灰度配置（D31）：forceRefresh 供入口打开时补拉（绕过 1h 快照缓存）。 */
  getOffPeakClientConfig(options?: ClientConfigReadOptions): Promise<OffPeakClientConfig>;
  /**
   * 动态工作流灰度快照：远端 `configs.dynamicWorkflow.mode` 与本地覆盖折叠后的结果；
   * forceRefresh 绕过 1h 快照缓存。请求失败 fail-closed（disabled/default）。
   */
  getDynamicWorkflowClientConfig(
    options?: ClientConfigReadOptions,
  ): Promise<DynamicWorkflowClientConfig>;
}

export const IClientConfigService = createServiceDescriptor<IClientConfigService>(
  ServiceChannels.ClientConfig,
);
