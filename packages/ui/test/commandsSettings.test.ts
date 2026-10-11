import { describe, expect, it } from "vitest";
import {
  COMMAND_AGENT_SOURCES,
  COMMAND_AGENT_SOURCE_META,
} from "../src/settings/commandsSettingsShared.js";

describe("commands settings source filter", () => {
  it("keeps built-in command sources in stable display order", () => {
    expect(COMMAND_AGENT_SOURCES).toEqual(["escodeAgent"]);
  });

  it("maps command sources to user-facing metadata", () => {
    expect(COMMAND_AGENT_SOURCE_META.escodeAgent).toMatchObject({
      labelId: "settings.commands.source.escodeAgent",
      pathHint: "~/.escode/commands",
      provider: "glm",
    });
  });
});
