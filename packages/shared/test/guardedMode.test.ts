import { describe, expect, it } from "vitest";
import {
  getESCodeAgentAvailableModes,
  getESCodeAgentModeSelectOptions,
  normalizeAvailableESCodeMode,
} from "../src/escode-agent-model-state.js";
import { submissionModeSchema } from "../src/escode-protocol-v4/submission.js";
import { escodeSessionModeSchema } from "../src/escode-protocol-legacy-types.js";
import { executionStateSchema, resolveExecutionState } from "../src/execution-state.js";

describe("guarded wire and mode catalog", () => {
  it("preserves guarded across independent Plan transitions and serialization", () => {
    const current = resolveExecutionState({ mode: "guarded", planEnabled: true });
    expect(executionStateSchema.parse(current)).toEqual({ mode: "guarded", planEnabled: true });
    expect(resolveExecutionState({ planEnabled: false }, current)).toEqual({
      mode: "guarded",
      planEnabled: false,
    });
  });
  it("round trips the opt-in value without changing the default", () => {
    expect(submissionModeSchema.parse("guarded")).toBe("guarded");
    expect(escodeSessionModeSchema.parse("guarded")).toBe("guarded");
    expect(normalizeAvailableESCodeMode("guarded")).toBe("guarded");
    const ids = getESCodeAgentAvailableModes().map((mode) => mode.id);
    expect(ids).toEqual(["build", "guarded", "plan", "yolo"]);
    expect(getESCodeAgentModeSelectOptions().map((option) => option.value)).toEqual(ids);
    expect(submissionModeSchema.safeParse("unknown").success).toBe(false);
  });
  it("keeps historical edit valid without advertising it as a new choice", () => {
    expect(submissionModeSchema.parse("edit")).toBe("edit");
    expect(escodeSessionModeSchema.parse("edit")).toBe("edit");
    expect(normalizeAvailableESCodeMode("edit")).toBe("edit");
    expect(getESCodeAgentAvailableModes().some((mode) => mode.id === "edit")).toBe(false);
  });
});
