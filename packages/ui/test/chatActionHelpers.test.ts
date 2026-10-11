import { describe, expect, it } from "vitest";
import type { ESCodeConfigOption } from "@escode/shared";
import { readCurrentESCodeMode } from "../src/hooks/chatActionHelpers.js";

function createModeOption(currentValue: string | boolean): ESCodeConfigOption {
  return {
    id: "mode",
    name: "Mode",
    category: "mode",
    type: "select",
    currentValue,
  };
}

describe("chatActionHelpers", () => {
  it("reads yolo from ESCode Agent mode config options", () => {
    expect(readCurrentESCodeMode([createModeOption("yolo")])).toBe("yolo");
  });

  it("reads edit from ESCode Agent mode config options", () => {
    expect(readCurrentESCodeMode([createModeOption("edit")])).toBe("edit");
  });

  it("keeps provider-native ESCode Agent modes", () => {
    expect(readCurrentESCodeMode([createModeOption("bypassPermissions")])).toBe("bypassPermissions");
  });

  it("does not normalize retired provider-native agent mode ids", () => {
    const readMode = readCurrentESCodeMode as (
      options: readonly ESCodeConfigOption[],
      provider?: string,
    ) => unknown;
    expect(readCurrentESCodeMode([createModeOption("agent")])).toBeUndefined();
    expect(readMode([createModeOption("agent-full-access")], "gemini")).toBeUndefined();
  });

  it("ignores invalid mode values", () => {
    expect(readCurrentESCodeMode([createModeOption("unknown")])).toBeUndefined();
    expect(readCurrentESCodeMode([createModeOption(true)])).toBeUndefined();
  });
});
