import { describe, expect, it } from "vitest";
import { ESCODE_COMPATIBILITY_MANIFEST, helloMessageSchema } from "@escode/shared";

const legacyHello = {
  type: "escode-hello" as const,
  version: "3.15.2",
  platform: "linux",
  arch: "x64",
  pid: 4321,
};

describe("stdio hello compatibility manifest", () => {
  it("accepts the optional compatibility manifest emitted by the server", () => {
    const parsed = helloMessageSchema.parse({
      ...legacyHello,
      compatibility: ESCODE_COMPATIBILITY_MANIFEST,
    });
    expect(parsed.compatibility).toEqual(ESCODE_COMPATIBILITY_MANIFEST);
  });

  it("keeps legacy hellos without compatibility valid", () => {
    expect(helloMessageSchema.parse(legacyHello)).toEqual(legacyHello);
  });

  it("rejects an unknown compatibility schemaVersion", () => {
    expect(() =>
      helloMessageSchema.parse({
        ...legacyHello,
        compatibility: {
          schemaVersion: 2,
          productVersion: "3.15.2",
          minCompatibleVersion: "3.15.2",
        },
      }),
    ).toThrow();
  });
});
