import { z } from "zod";
import type { CommandAgentSource } from "./command-types.js";
import type { ESCodeProvider } from "./escode-task-types-core.js";

export const ESCODE_AGENT_PROVIDER = "glm" satisfies ESCodeProvider;
export const ESCODE_AGENT_PROVIDER_LABEL = "ESCode Agent";
export const ESCODE_COMMAND_AGENT_SOURCE = "escodeAgent" satisfies CommandAgentSource;

/**
 * 解析持久化/relay 数据时的 provider 归一 schema：
 * 已移除的第三方 provider 历史值仅作为归一化输入保留，统一收敛为 glm；
 * 其它非法值保持拒绝，与收窄前的校验边界一致。
 */
export const legacyESCodeProviderSchema = z
  .enum(["claude", "opencode", "gemini", "codex", "glm"])
  .transform((): typeof ESCODE_AGENT_PROVIDER => ESCODE_AGENT_PROVIDER);

export const ESCODE_ENABLED_AGENT_PROVIDERS = [
  ESCODE_AGENT_PROVIDER,
] as const satisfies readonly ESCodeProvider[];

export const ESCODE_COMMAND_AGENT_SOURCES = [
  ESCODE_COMMAND_AGENT_SOURCE,
] as const satisfies readonly CommandAgentSource[];

export function normalizeAgentProviderToESCodeAgent(
  _provider?: ESCodeProvider | null,
): ESCodeProvider {
  return ESCODE_AGENT_PROVIDER;
}

export function normalizeAgentProvidersToESCodeAgent(
  _providers?: ReadonlyArray<ESCodeProvider> | null,
): ESCodeProvider[] {
  return [...ESCODE_ENABLED_AGENT_PROVIDERS];
}

export function normalizeCommandAgentSourceToESCodeAgent(
  _agentSource?: CommandAgentSource | null,
): CommandAgentSource {
  return ESCODE_COMMAND_AGENT_SOURCE;
}

export function isESCodeAgentProvider(
  provider: ESCodeProvider | null | undefined,
): provider is typeof ESCODE_AGENT_PROVIDER {
  return provider === ESCODE_AGENT_PROVIDER;
}
