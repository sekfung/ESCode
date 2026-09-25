// Run with node --import tsx. 以 TS 的 embedded search 后端解析与 Bash prelude 生成为 oracle，
// 导出 环境变量 × shell 方言 语料，Rust tools::embedded_search 逐字比对（--check 防漂移）。
// 见 docs/specs/rust-tool-surface.md。
import { readFile, writeFile } from "node:fs/promises";
import { resolveDefaultEmbeddedSearchBackend } from "../apps/zcode-cli/packages/bootstrap/src/app/embedded-search-backend.ts";
import { buildEmbeddedSearchPreludeContent } from "../apps/zcode-cli/packages/adapters/src/exec/embedded-search-prelude.ts";

const environments = [
  {},
  {
    ZCODE_BFS_BINARY: "/opt/zcode/bfs",
    ZCODE_UGREP_BINARY: "/opt/zcode/ugrep",
    ZCODE_RG_BINARY: "/opt/zcode/rg",
  },
  {
    ZCODE_BFS_BINARY: "C:\\Program Files\\ZCode\\bfs.exe",
    ZCODE_UGREP_BINARY: "C:\\Program Files\\ZCode\\ugrep.exe",
    ZCODE_RG_BINARY: "C:\\Program Files\\ZCode\\rg.exe",
  },
  { ZCODE_UGREP_BINARY: "  /it's/ugrep  ", ZCODE_RG_BINARY: "rg" },
  { ZCODE_RG_BINARY: "\\\\server\\share\\rg.exe" },
  { ZCODE_EMBEDDED_SEARCH_COMMAND: "/opt/zcode/zcode" },
  { ZCODE_EMBEDDED_SEARCH_COMMAND: "C:\\ZCode\\zcode.exe", ZCODE_BFS_BINARY: "/ignored" },
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
  "../apps/zcode-cli-rust/crates/tools/tests/fixtures/embedded_search_prelude.json",
  import.meta.url,
);
if (process.argv.includes("--check")) {
  if ((await readFile(target, "utf8")) !== content)
    throw new Error("Rust embedded search prelude corpus differs from TS");
} else await writeFile(target, content);
