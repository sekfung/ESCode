// Run with node --import tsx. 以 TS 的 embedded search 后端解析与 Bash prelude 生成为 oracle，
// 导出 环境变量 × shell 方言 语料，Rust tools::embedded_search 逐字比对（--check 防漂移）。
// 见 docs/specs/rust-tool-surface.md。
import { readFile, writeFile } from "node:fs/promises";
import { resolveDefaultEmbeddedSearchBackend } from "../apps/escode-cli/packages/bootstrap/src/app/embedded-search-backend.ts";
import { buildEmbeddedSearchPreludeContent } from "../apps/escode-cli/packages/adapters/src/exec/embedded-search-prelude.ts";

const environments = [
  {},
  {
    ESCODE_BFS_BINARY: "/opt/escode/bfs",
    ESCODE_UGREP_BINARY: "/opt/escode/ugrep",
    ESCODE_RG_BINARY: "/opt/escode/rg",
  },
  {
    ESCODE_BFS_BINARY: "C:\\Program Files\\ESCode\\bfs.exe",
    ESCODE_UGREP_BINARY: "C:\\Program Files\\ESCode\\ugrep.exe",
    ESCODE_RG_BINARY: "C:\\Program Files\\ESCode\\rg.exe",
  },
  { ESCODE_UGREP_BINARY: "  /it's/ugrep  ", ESCODE_RG_BINARY: "rg" },
  { ESCODE_RG_BINARY: "\\\\server\\share\\rg.exe" },
  { ESCODE_EMBEDDED_SEARCH_COMMAND: "/opt/escode/escode" },
  { ESCODE_EMBEDDED_SEARCH_COMMAND: "C:\\ESCode\\escode.exe", ESCODE_BFS_BINARY: "/ignored" },
];
const dialects = ["posix", "git-bash", "cmd", "legacy-shell"];
const cases = [];
for (const env of environments)
  for (const dialect of dialects) {
    const backend = resolveDefaultEmbeddedSearchBackend({ env });
    const content = buildEmbeddedSearchPreludeContent(
      { kind: "embedded-search", backend },
      { shellDialect: dialect },
    );
    cases.push({ env, dialect, content: content ?? null });
  }

const content = `${JSON.stringify({ cases }, null, 1)}\n`;
const target = new URL(
  "../apps/escode-cli-rust/crates/tools/tests/fixtures/embedded_search_prelude.json",
  import.meta.url,
);
if (process.argv.includes("--check")) {
  if ((await readFile(target, "utf8")) !== content)
    throw new Error("Rust embedded search prelude corpus differs from TS");
} else await writeFile(target, content);
