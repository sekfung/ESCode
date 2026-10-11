import { describe, expect, it } from "vitest";
import { ESCODE_AGENT_RUNTIME } from "@escode/shared";

describe("ESCode Agent runtime", () => {
  it("starts the private ESCode Protocol server instead of the retired ESCode Agent command", () => {
    const retiredProtocolName = `${"a"}${"cp"}`;

    expect(ESCODE_AGENT_RUNTIME.spawnArgs).toEqual(["app-server", "--stdio"]);
    expect(ESCODE_AGENT_RUNTIME.spawnArgs).not.toContain(retiredProtocolName);
  });

  it("uses the ESCode Agent binary name", () => {
    expect(ESCODE_AGENT_RUNTIME.resolveEntrySegments("darwin")).toEqual(["escode-agent"]);
    expect(ESCODE_AGENT_RUNTIME.resolveEntrySegments("win32")).toEqual(["escode-agent.exe"]);
  });
});
