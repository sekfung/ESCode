import { z } from "zod";
import type { CommandAgentSource } from "./command-types.js";
import type { ESCodeProvider } from "./escode-task-types-core.js";

export const ESCODE_AGENT_PROVIDER = "glm" satisfies ESCodeProvider;
export const ESCODE_AGENT_PROVIDER_LABEL = "ESCode Agent";
export const ESCODE_COMMAND_AGENT_SOURCE = "escodeAgent" satisfies CommandAgentSource;

export const escodeAgentProviderSchema = z.literal(ESCODE_AGENT_PROVIDER);

export const ESCODE_COMMAND_AGENT_SOURCES = [
  ESCODE_COMMAND_AGENT_SOURCE,
] as const satisfies readonly CommandAgentSource[];

export function normalizeAgentProviderToESCodeAgent(
  _provider?: ESCodeProvider | null,
): ESCodeProvider {
  return ESCODE_AGENT_PROVIDER;
}

export function isESCodeAgentProvider(
  provider: ESCodeProvider | null | undefined,
): provider is typeof ESCODE_AGENT_PROVIDER {
  return provider === ESCODE_AGENT_PROVIDER;
}
