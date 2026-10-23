//! 资源管理器 Agent 注册表兜底行（从 `resourceManagerWindow.ts` 拆出以守住 max-lines）。

import { formatESCodeAgentProcessName, type ResourceUsageProcess } from "@escode/shared";

interface RegisteredAgentProcessLike {
  pid: number;
  provider: string;
  workspacePath: string;
  startedAt: number;
}

/** Host 还没回报采样的 Agent 也要出现在列表里（指标未采到）。 */
export function collectUnsampledAgentProcesses(
  hostAgentProcesses: Map<string, Map<number, RegisteredAgentProcessLike>>,
  sampledPids: Set<number>,
): ResourceUsageProcess[] {
  const rows: ResourceUsageProcess[] = [];
  for (const processes of hostAgentProcesses.values()) {
    for (const agent of [...processes.values()].sort(
      (left, right) => left.startedAt - right.startedAt,
    )) {
      if (sampledPids.has(agent.pid)) continue;
      rows.push({
        pid: agent.pid,
        name: formatESCodeAgentProcessName(agent.provider as never, agent.workspacePath),
        category: "base",
        groupKey: "cli",
        groupLabel: "cli",
        cpuPercent: 0,
        memoryBytes: 0,
        sampled: false,
      });
    }
  }
  return rows;
}
