import { describe, expect, it } from "vitest";

import {
  decodeMarkdownArtifactImageSource,
  extractMarkdownArtifactImageRefs,
  rewriteMarkdownArtifactImageSources,
} from "../src/markdown-artifact-images.js";

describe("Markdown artifact image references", () => {
  it("extracts only image destinations and deduplicates them", () => {
    expect(
      extractMarkdownArtifactImageRefs(
        [
          "![first](escode-artifact://session/a)",
          "[download](escode-artifact://session/not-an-image)",
          '![again](<escode-artifact://session/a> "preview")',
          "![second](escode-artifact://session/b 'title')",
        ].join("\n"),
      ),
    ).toEqual(["escode-artifact://session/a", "escode-artifact://session/b"]);
  });

  it("does not authorize image-looking text inside fenced code", () => {
    expect(
      extractMarkdownArtifactImageRefs(
        "```md\n![secret](escode-artifact://session/secret)\n```\n![ok](escode-artifact://session/ok)",
      ),
    ).toEqual(["escode-artifact://session/ok"]);
  });

  it("rewrites artifact image destinations to a harden-safe relative source", () => {
    const rewritten = rewriteMarkdownArtifactImageSources(
      "before ![shot](escode-artifact://session/shot-1) after",
    );
    expect(rewritten).not.toContain("escode-artifact://");
    const source = rewritten.match(/!\[shot\]\(([^)]+)\)/u)?.[1];
    expect(source).toBeTruthy();
    expect(decodeMarkdownArtifactImageSource(source ?? "")).toBe(
      "escode-artifact://session/shot-1",
    );
  });

  it("leaves normal images, links, and fenced examples unchanged", () => {
    const markdown = [
      "![web](https://example.com/image.png)",
      "[artifact](escode-artifact://session/file)",
      "```md",
      "![example](escode-artifact://session/example)",
      "```",
    ].join("\n");
    expect(rewriteMarkdownArtifactImageSources(markdown)).toBe(markdown);
  });
});
