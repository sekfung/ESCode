import { describe, expect, it } from "vitest";
import { bundledLanguages, bundledLanguagesInfo } from "shiki";
import {
  SHIKI_LANGUAGE_ALIASES,
  SHIKI_LANGUAGE_IDS,
} from "@/lib/shikiLanguageRegistry.generated.js";
import {
  createRawCodeTokens,
  highlightCode,
  shouldUseSyntaxHighlighting,
} from "@/lib/shikiHighlighter.js";

describe("shikiHighlighter", () => {
  it("keeps plain text code fences out of the async highlighter", () => {
    expect(shouldUseSyntaxHighlighting("text")).toBe(false);
    expect(shouldUseSyntaxHighlighting("plaintext")).toBe(false);
    expect(shouldUseSyntaxHighlighting("log")).toBe(false);
    expect(shouldUseSyntaxHighlighting("not-a-real-language")).toBe(false);
  });

  it("allows known languages and aliases to use syntax highlighting", () => {
    expect(shouldUseSyntaxHighlighting("json")).toBe(true);
    expect(shouldUseSyntaxHighlighting("typescript")).toBe(true);
    expect(shouldUseSyntaxHighlighting("tsx")).toBe(true);
  });

  it("generated language registry matches the shiki bundle", () => {
    expect([...SHIKI_LANGUAGE_IDS].sort()).toEqual(Object.keys(bundledLanguages).sort());
    const aliases: Record<string, string> = {};
    for (const info of bundledLanguagesInfo) {
      for (const alias of info.aliases ?? []) {
        if (!Object.hasOwn(aliases, alias)) aliases[alias] = info.id;
      }
    }
    expect(SHIKI_LANGUAGE_ALIASES).toEqual(
      Object.fromEntries(Object.entries(aliases).sort(([a], [b]) => a.localeCompare(b))),
    );
  });

  it("returns raw tokens for plain text without registering async callbacks", () => {
    let callbackCount = 0;
    const code = [
      '"next start" does not work with "output: standalone" configuration.',
      'Use "node .next/standalone/server.js" instead.',
    ].join("\n");

    const tokenized = highlightCode(code, "text", "github-light", () => {
      callbackCount += 1;
    });

    expect(tokenized).toEqual(createRawCodeTokens(code));
    expect(callbackCount).toBe(0);
  });
});
