import { describe, expect, it, vi } from "vitest";
import type { AppUsageSnapshot } from "@escode/shared";
import type { IESCodeAgentService } from "../src/escode-agent/escodeAgent.js";
import { createUsageStatsService } from "../src/usage-stats/usageStatsService.js";

// 套餐族删除（2026-10-15）后 usage-stats 只保留 App Usage（agent 数据库统计）。
// Coding Plan monitor / quota reset / entitlement 的用例随服务一并移除。

function createAppUsageSnapshot(): AppUsageSnapshot {
  return {
    range: "30d",
    generatedAt: 1_900_000_000_000,
    timeZone: "UTC",
    source: "agent-db",
    summary: {
      totalTokens: 104,
      inputTokens: 14,
      outputTokens: 5,
      reasoningTokens: 0,
      cacheCreationTokens: 5,
      cacheReadTokens: 80,
      cacheHitRate: 0.8,
      totalSessions: 1,
      totalTurns: 1,
      toolCallCount: 2,
      toolErrorRate: 0.5,
      modelErrorRate: 0.5,
      avgTimeToFirstTokenMs: 100,
      avgTurnDurationMs: 2000,
      activeDays: 2,
      currentStreakDays: 2,
      longestSessionMs: 2000,
      longestStreakDays: 2,
      peakDayTokens: 104,
      favoriteModel: { modelId: "model-a", totalTokens: 104, share: 1 },
    },
    heatmap: { startDate: null, endDate: null, maxTokens: 100, weeks: [] },
    dailyModelUsage: [],
    models: [],
    tools: [],
  };
}

function createUsageStatsServiceForTest(getAppUsageStats: () => Promise<AppUsageSnapshot>) {
  return createUsageStatsService({
    escodeAgentService: {
      getAppUsageStats,
    } as unknown as Pick<IESCodeAgentService, "getAppUsageStats">,
  });
}

describe("createUsageStatsService（App Usage）", () => {
  it("App Usage 经 ESCode Protocol 读取 agent 数据库统计", async () => {
    const getAppUsageStats = vi.fn(async () => createAppUsageSnapshot());
    const service = createUsageStatsServiceForTest(getAppUsageStats);

    const snapshot = await service.getAppUsageSnapshot({
      range: "30d",
      timeZone: "UTC",
    });

    expect(getAppUsageStats).toHaveBeenCalledWith({
      range: "30d",
      timeZone: "UTC",
    });
    expect(snapshot.source).toBe("agent-db");
    expect(snapshot.summary.totalSessions).toBe(1);
    expect(snapshot.summary.totalTokens).toBe(104);
  });

  it("App Usage 全量概览用 all range 透传到 agent 数据库统计", async () => {
    const getAppUsageStats = vi.fn(async () => ({
      ...createAppUsageSnapshot(),
      range: "all" as const,
    }));
    const service = createUsageStatsServiceForTest(getAppUsageStats);

    const snapshot = await service.getAppUsageSnapshot({
      range: "all",
      timeZone: "Asia/Shanghai",
    });

    expect(getAppUsageStats).toHaveBeenCalledWith({
      range: "all",
      timeZone: "Asia/Shanghai",
    });
    expect(snapshot.range).toBe("all");
  });
});
