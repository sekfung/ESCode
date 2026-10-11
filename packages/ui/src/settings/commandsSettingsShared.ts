import {
  ESCODE_AGENT_PROVIDER,
  ESCODE_COMMAND_AGENT_SOURCES,
  type ESCodeProvider,
  type CommandAgentSource,
} from "@escode/shared";

export const COMMAND_AGENT_SOURCES = [
  ...ESCODE_COMMAND_AGENT_SOURCES,
] as const satisfies readonly CommandAgentSource[];

export const COMMAND_AGENT_SOURCE_META: Record<
  CommandAgentSource,
  {
    labelId: string;
    provider: ESCodeProvider;
    pathHint: string;
  }
> = {
  escodeAgent: {
    labelId: "settings.commands.source.escodeAgent",
    pathHint: "~/.escode/commands",
    provider: ESCODE_AGENT_PROVIDER,
  },
};
