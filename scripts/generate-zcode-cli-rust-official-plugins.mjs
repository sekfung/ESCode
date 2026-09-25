// Run with node --import tsx. 官方插件 seed 的 Rust 资产（docs/specs/rust-official-plugin-seed.md）：
// 插件定义、文件白名单、ASCII 排序表（TS 用 localeCompare 排序文件清单，插件 hash 依赖该顺序），
// 以及排序语料（Rust 单测逐条比对）。--check 防漂移。
import { readFile, writeFile } from "node:fs/promises";
import { format } from "oxfmt";
import {
  ZCODE_OFFICIAL_PLUGIN_MARKETPLACE,
  ZCODE_PLUGIN_HOST_COMMAND,
} from "../apps/zcode-cli/packages/contracts/src/plugins/index.ts";
import { OFFICIAL_PLUGIN_DEFINITIONS } from "../apps/zcode-cli/packages/bootstrap/src/app/official-plugin-definitions.ts";
import { ZCODE_PLUGIN_ID_ENV_KEY } from "../packages/shared/src/mcp.ts";

// TS 未导出该白名单：从源码读取同一个常量，避免手抄漂移。
const source = await readFile(
  new URL("../apps/zcode-cli/packages/bootstrap/src/app/bundled-plugins.ts", import.meta.url),
  "utf8",
);
const block = source.match(/const includedTopLevelPaths = new Set\(\[([\s\S]*?)\]\);/);
if (!block) throw new Error("includedTopLevelPaths not found in bundled-plugins.ts");
const includedTopLevelPaths = [...block[1].matchAll(/"([^"]+)"/g)].map((match) => match[1]);

// ASCII 可见字符按 localeCompare 的一级顺序；大小写同一级（小写在前），由 Rust 以三级比较区分。
const ascii = Array.from({ length: 0x7f - 0x20 }, (_, index) => String.fromCharCode(0x20 + index));
const collation = ascii
  .filter((char) => !/[A-Z]/.test(char))
  .sort((left, right) => left.localeCompare(right));

const samples = [
  "skills/control-browser/SKILL.md",
  "skills/Control/SKILL.md",
  "skills/control_browser/a.md",
  "skills/control-browser.md",
  "skills/controlbrowser/a.md",
  ".zcode-plugin/plugin.json",
  ".mcp.json",
  "README.md",
  "readme.md",
  "Readme.md",
  "agents/visual-judge.md",
  "agents/visual_judge.md",
  "agents/visualjudge.md",
  "dist/mcp/server.js",
  "dist/mcp/server.js.map",
  "dist/mcp/Server.js",
  "docs/api.json",
  "docs/api-v2.json",
  "docs/api.v2.json",
  "docs/api10.json",
  "docs/api9.json",
  "docs/API.json",
  "hooks/pre-tool.sh",
  "hooks/pre_tool.sh",
  "hooks/pre tool.sh",
  "scripts/browser-client.mjs",
  "scripts/browser~client.mjs",
  "scripts/browser+client.mjs",
  "scripts/browser@client.mjs",
  "templates/a/b/c.txt",
  "templates/a/b-c.txt",
  "templates/a/bc.txt",
  "templates/a/b.c.txt",
  "package.json",
  "output-styles/x.md",
  "commands/workflow.md",
  "commands/work-flow.md",
];
const sorted = [...samples].sort((left, right) => left.localeCompare(right));

const assets = {
  marketplace: ZCODE_OFFICIAL_PLUGIN_MARKETPLACE,
  hostCommand: ZCODE_PLUGIN_HOST_COMMAND,
  pluginIdEnvKey: ZCODE_PLUGIN_ID_ENV_KEY,
  includedTopLevelPaths,
  collation,
  definitions: OFFICIAL_PLUGIN_DEFINITIONS.map((definition) => ({
    name: definition.name,
    version: definition.version,
    rootCandidates: definition.rootCandidates,
    requiredSeedPaths: definition.requiredSeedPaths ?? [],
    runtimeTopLevelPaths: definition.runtimeTopLevelPaths ?? [],
    ...(definition.listing ? { listing: definition.listing } : {}),
  })),
};

const outputs = [
  ["../apps/zcode-cli-rust/crates/tools/src/official_plugins.json", assets],
  [
    "../apps/zcode-cli-rust/crates/tools/tests/fixtures/official_plugin_sort.json",
    { samples, sorted },
  ],
];
for (const [file, data] of outputs) {
  const path = new URL(file, import.meta.url);
  const formatted = await format(path.pathname, `${JSON.stringify(data, null, 2)}\n`);
  if (formatted.errors.length) throw new Error(`Cannot format ${file}`);
  if (process.argv.includes("--check")) {
    if ((await readFile(path, "utf8")) !== formatted.code)
      throw new Error(`Rust official plugin asset differs from TS: ${file}`);
  } else await writeFile(path, formatted.code);
}
