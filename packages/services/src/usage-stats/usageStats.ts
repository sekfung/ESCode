import type { AppUsageRequest, AppUsageSnapshot } from "@escode/shared";
import { ServiceChannels } from "@escode/shared";
import { createServiceDescriptor } from "../descriptors.js";

/**
 * 用量统计面。Coding Plan monitor / quota reset / entitlement 链路随套餐族删除（2026-10-15），
 * 只保留读取本地 agent 数据库统计的 App Usage。
 */
export interface IUsageStatsService {
  getAppUsageSnapshot(request: AppUsageRequest): Promise<AppUsageSnapshot>;
}

export const IUsageStatsService = createServiceDescriptor<IUsageStatsService>(
  ServiceChannels.UsageStats,
);
