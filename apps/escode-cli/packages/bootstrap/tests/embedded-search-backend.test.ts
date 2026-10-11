import { describe, expect, it } from "vitest";
import { resolveDefaultEmbeddedSearchBackend } from "../src/app/embedded-search-backend.js";

describe("resolveDefaultEmbeddedSearchBackend", () => {
  it("uses bundled bfs, ugrep, and ripgrep paths by default", () => {
    expect(
      resolveDefaultEmbeddedSearchBackend({
        env: {
          ESCODE_BFS_BINARY: "/opt/escode/tools/bfs/bfs",
          ESCODE_RG_BINARY: "/opt/escode/tools/ripgrep/rg",
          ESCODE_UGREP_BINARY: "/opt/escode/tools/ugrep/ugrep",
        },
      }),
    ).toEqual({
      kind: "native-binaries",
      findCommand: "/opt/escode/tools/bfs/bfs",
      grepCommand: "/opt/escode/tools/ugrep/ugrep",
      rgCommand: "/opt/escode/tools/ripgrep/rg",
    });
  });

  it("falls back to bfs, ugrep, and rg command names when bundled paths are unavailable", () => {
    expect(resolveDefaultEmbeddedSearchBackend({ env: {} })).toEqual({
      kind: "native-binaries",
      findCommand: "bfs",
      grepCommand: "ugrep",
      rgCommand: "rg",
    });
  });

  it("preserves the explicit internal CLI compatibility override", () => {
    expect(
      resolveDefaultEmbeddedSearchBackend({
        env: { ESCODE_EMBEDDED_SEARCH_COMMAND: "/tmp/escode-test" },
      }),
    ).toEqual({
      kind: "internal-cli",
      command: "/tmp/escode-test",
      args: ["__internal-search"],
    });
  });
});
