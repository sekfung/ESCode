import { describe, expect, it } from "vitest";
import {
  consumeEscodeJwtInvalidRestartMarker,
  markEscodeJwtInvalidRestart,
} from "../src/root/escodeJwtInvalidRestartMarker.js";

function createStorage() {
  const values = new Map<string, string>();
  return {
    getItem: (key: string) => values.get(key) ?? null,
    removeItem: (key: string) => values.delete(key),
    setItem: (key: string, value: string) => values.set(key, value),
  };
}

describe("escode JWT invalid restart marker", () => {
  it("is consumed exactly once after relaunch", () => {
    const storage = createStorage();
    markEscodeJwtInvalidRestart(storage);

    expect(consumeEscodeJwtInvalidRestartMarker(storage)).toBe(true);
    expect(consumeEscodeJwtInvalidRestartMarker(storage)).toBe(false);
  });
});
