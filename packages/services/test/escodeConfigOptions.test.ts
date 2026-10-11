import { describe, expect, it } from "vitest";
import type { ESCodeSessionSettingsState } from "@escode/shared";
import {
  getESCodeAgentAvailableModes,
  normalizeAvailableESCodeMode,
  settingsToConfigOptions,
} from "../src/escode-agent/escodeConfigOptions.js";

function settingsWithMode(
  mode: ESCodeSessionSettingsState["mode"]["current"],
): ESCodeSessionSettingsState {
  return {
    model: {
      current: { providerId: "glm", modelId: "glm-5" },
      available: [],
    },
    thoughtLevel: {
      enabled: false,
      available: [],
    },
    mode: { current: mode },
  };
}

describe("escodeConfigOptions", () => {
  it("keeps persisted edit while the shared menu replaces it with guarded", () => {
    const options = settingsToConfigOptions(settingsWithMode("edit"));
    const modeOption = options.find((option) => option.category === "mode");

    expect(modeOption?.currentValue).toBe("edit");
    expect(modeOption?.options?.map((option) => option.value)).toEqual([
      "build",
      "guarded",
      "plan",
      "yolo",
    ]);
    expect(getESCodeAgentAvailableModes().map((mode) => mode.id)).toEqual([
      "build",
      "guarded",
      "plan",
      "yolo",
    ]);
    expect(normalizeAvailableESCodeMode("edit")).toBe("edit");
  });
});
