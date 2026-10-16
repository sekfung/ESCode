import type { AppUsageRequest, AppUsageSnapshot } from "@escode/shared";
import type { IESCodeAgentService } from "../escode-agent/escodeAgent.js";
import type { IUsageStatsService } from "./usageStats.js";

/**
 * 用量统计服务。Coding Plan monitor / quota reset / entitlement 链路随套餐族删除
 * （2026-10-15，BigModelUsageQuotaProvider 一并移除）；App Usage 读取 agent 数据库真实统计。
 */
export function createUsageStatsService(dependencies: {
  /** App Usage 经 ESCode Protocol 读取 agent 数据库真实统计。 */
  escodeAgentService: Pick<IESCodeAgentService, "getAppUsageStats">;
}): IUsageStatsService {
  return {
    async getAppUsageSnapshot(request: AppUsageRequest): Promise<AppUsageSnapshot> {
      // App Usage 读取 agent 数据库真实统计（model_usage/turn_usage/tool_usage），
      // 经 ESCode Protocol usage/stats 取回。
      return dependencies.escodeAgentService.getAppUsageStats({
        range: request.range,
        timeZone: request.timeZone,
      });
    },
  };
}
