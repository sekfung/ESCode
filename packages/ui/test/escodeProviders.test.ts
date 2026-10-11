import { describe, expect, it } from "vitest";
import {
  shouldHideESCodeSessionForkAction,
  supportsESCodeSessionFork,
} from "../src/lib/escodeProviders.js";

describe("supportsESCodeSessionFork", () => {
  it("所有 provider 都开放 session fork 入口", () => {
    expect(supportsESCodeSessionFork("glm")).toBe(true);
    expect(supportsESCodeSessionFork("claude")).toBe(true);
    expect(supportsESCodeSessionFork("codex")).toBe(true);
    expect(supportsESCodeSessionFork("gemini")).toBe(true);
    expect(supportsESCodeSessionFork("opencode")).toBe(true);
  });
});

describe("shouldHideESCodeSessionForkAction", () => {
  it("所有 provider 都不再隐藏 fork UI 入口", () => {
    expect(shouldHideESCodeSessionForkAction("glm")).toBe(false);
    expect(shouldHideESCodeSessionForkAction("claude")).toBe(false);
    expect(shouldHideESCodeSessionForkAction("codex")).toBe(false);
    expect(shouldHideESCodeSessionForkAction("gemini")).toBe(false);
    expect(shouldHideESCodeSessionForkAction("opencode")).toBe(false);
  });
});
