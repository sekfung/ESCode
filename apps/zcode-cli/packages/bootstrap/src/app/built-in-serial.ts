import type { McpServerConfig, PluginLoadOutcome } from "@zcode/contracts";
import { SERIAL_MCP_SERVER_NAME, ZCODE_HOST_SERIAL_ENV } from "@zcode/shared/serial";
import { createBundledMcpRuntimeConfig } from "./official-plugin-runtime.js";
import { OFFICIAL_SERIAL_PLUGIN_ID } from "./official-plugin-definitions.js";

/** wait_for 上限 120s，加上 Host 往返余量。 */
const SERIAL_MCP_TIMEOUT_MS = 150_000;

/**
 * serial MCP server 由宿主注入（不在插件 manifest 中声明），因为 broker 连接材料只能运行时定向注入。
 * 只有 Host 声明本机拥有串口会话（Desktop Local Host 注入 ZCODE_HOST_SERIAL=1）且 serial 官方插件
 * 处于启用状态时才注册；远程 workspace / Web / 独立 CLI 不注册，模型看不到这组工具。
 */
export function isHostSerialAvailable(env: Record<string, string | undefined>): boolean {
  return env[ZCODE_HOST_SERIAL_ENV]?.trim() === "1";
}

export function resolveBuiltInSerialMcpServers(input: {
  /** 进程入口仅在 isHostSerialAvailable 时创建 broker；没有 broker 的 server 无法连回 Host。 */
  brokerAvailable: boolean;
  pluginOutcome: Pick<PluginLoadOutcome, "plugins">;
  workingDirectory: string;
}): Record<string, McpServerConfig> {
  if (!input.brokerAvailable) return {};
  const plugin = input.pluginOutcome.plugins.find(
    (candidate) => candidate.id === OFFICIAL_SERIAL_PLUGIN_ID && candidate.enabled,
  );
  if (!plugin) return {};
  const config = createBundledMcpRuntimeConfig({
    cwd: input.workingDirectory,
    env: { ZCODE_PLUGIN_ROOT: plugin.rootPath },
    rootPath: plugin.rootPath,
    timeoutMs: SERIAL_MCP_TIMEOUT_MS,
  });
  if (!config) return {};
  // serial server 只接受现代协商（serveStdio legacy: "reject"）：与 node_repl 一样固定协议版本，
  // 否则客户端按旧版 initialize 协商会被拒绝，串口工具整组不可用。
  return {
    [SERIAL_MCP_SERVER_NAME]: { ...config, isolation: "workspace", protocolVersion: "2026-07-28" },
  };
}
