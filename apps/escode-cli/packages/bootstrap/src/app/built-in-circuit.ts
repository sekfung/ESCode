import type { McpServerConfig, PluginLoadOutcome } from "@escode/contracts";
import { CIRCUIT_MCP_SERVER_NAME, ESCODE_HOST_CIRCUIT_ENV } from "@escode/shared/circuit";
import { createBundledMcpRuntimeConfig } from "./official-plugin-runtime.js";
import { OFFICIAL_CIRCUIT_PLUGIN_ID } from "./official-plugin-definitions.js";

/** 电路工具都是短请求，留出 Host 往返与 64 KiB 分页响应余量。 */
const CIRCUIT_MCP_TIMEOUT_MS = 60_000;

/**
 * circuit MCP server 由宿主注入（不在插件 manifest 中声明），因为 broker 连接材料只能运行时定向注入。
 * 只有 Host 声明自己持有 CircuitService（Desktop 本地 / 远程 workspace Host 注入 ESCODE_HOST_CIRCUIT=1）
 * 且 circuit 官方插件启用时才注册；独立 CLI、Web 客户端不注册，模型看不到这组工具（§3.5）。
 */
export function isHostCircuitAvailable(env: Record<string, string | undefined>): boolean {
  return env[ESCODE_HOST_CIRCUIT_ENV]?.trim() === "1";
}

export function resolveBuiltInCircuitMcpServers(input: {
  /** 进程入口仅在 isHostCircuitAvailable 时创建 broker；没有 broker 的 server 无法连回 Host。 */
  brokerAvailable: boolean;
  pluginOutcome: Pick<PluginLoadOutcome, "plugins">;
  workingDirectory: string;
}): Record<string, McpServerConfig> {
  if (!input.brokerAvailable) return {};
  const plugin = input.pluginOutcome.plugins.find(
    (candidate) => candidate.id === OFFICIAL_CIRCUIT_PLUGIN_ID && candidate.enabled,
  );
  if (!plugin) return {};
  const config = createBundledMcpRuntimeConfig({
    cwd: input.workingDirectory,
    env: { ESCODE_PLUGIN_ROOT: plugin.rootPath },
    rootPath: plugin.rootPath,
    timeoutMs: CIRCUIT_MCP_TIMEOUT_MS,
  });
  if (!config) return {};
  // 与 serial 一样固定现代协议版本：plugin 的 server 只接受现代协商（serveStdio legacy: "reject"），
  // 漏写会让客户端按旧版 initialize 协商被拒，整组电路工具不可用。
  return {
    [CIRCUIT_MCP_SERVER_NAME]: { ...config, isolation: "workspace", protocolVersion: "2026-07-28" },
  };
}
