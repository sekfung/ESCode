// 生成 packages/ui/src/lib/shikiLanguageRegistry.generated.ts。
//
// 目的：`shouldUseSyntaxHighlighting` 需要在首屏同步判断语言是否受支持，但直接 import
// `bundledLanguages` 会把 shiki 的语言/语法包拉进 renderer 初始模块图。这里把语言 id 与
// alias 冻结成纯字符串表；真实高亮仍由 `import("shiki")` 按需加载。
//
// 用法：node scripts/generate-ui-shiki-language-registry.mjs [--check]
import { writeFile } from "node:fs/promises";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { bundledLanguages, bundledLanguagesInfo } from "shiki";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const target = resolve(root, "packages/ui/src/lib/shikiLanguageRegistry.generated.ts");

const ids = Object.keys(bundledLanguages).sort();
const aliases = {};
for (const info of bundledLanguagesInfo) {
  for (const alias of info.aliases ?? []) {
    if (!Object.hasOwn(aliases, alias)) aliases[alias] = info.id;
  }
}
const sortedAliases = Object.fromEntries(
  Object.entries(aliases).sort(([left], [right]) => left.localeCompare(right)),
);

const lines = [
  "/* oxlint-disable max-lines -- 由 scripts/generate-ui-shiki-language-registry.mjs 生成的语言表，按 shiki 全量 id 机械展开。 */",
  "// 由 scripts/generate-ui-shiki-language-registry.mjs 生成；请勿手改。",
  "// 目的：让 shouldUseSyntaxHighlighting 在首屏同步判断语言时不必把整个 shiki 语言包",
  "// （数 MB）打进 renderer 初始模块图。漂移由 ui/test/shikiHighlighter.test.ts 守护。",
  "",
  "export const SHIKI_LANGUAGE_IDS: readonly string[] = [",
  ...ids.map((id) => `  ${JSON.stringify(id)},`),
  "];",
  "",
  "export const SHIKI_LANGUAGE_ALIASES: Readonly<Record<string, string>> = {",
  ...Object.entries(sortedAliases).map(
    ([alias, id]) => `  ${JSON.stringify(alias)}: ${JSON.stringify(id)},`,
  ),
  "};",
  "",
];

const content = lines.join("\n");
if (process.argv.includes("--check")) {
  const current = await readFile(target, "utf8");
  if (current !== content) {
    console.error(
      `${target} is out of date; run node scripts/generate-ui-shiki-language-registry.mjs`,
    );
    process.exit(1);
  }
} else {
  await writeFile(target, content);
  console.log(`wrote ${target}`);
}
