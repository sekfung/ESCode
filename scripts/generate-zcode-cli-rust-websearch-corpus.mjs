// Run with node --import tsx. WebSearch 纯规则的 TS oracle（docs/specs/rust-websearch.md）：
// 描述（固定日期）、输入 JSON schema、流式收集后的输出与模型可见内容。--check 防漂移。
import { readFile, writeFile } from "node:fs/promises";
import { WebSearchInputJsonSchema } from "../apps/zcode-cli/packages/contracts/src/tools/websearch.ts";
import { buildWebSearchProviderDescription } from "../apps/zcode-cli/packages/core/src/tool/handlers/websearch.ts";
import {
  buildWebSearchOutput,
  formatWebSearchModelContent,
} from "../apps/zcode-cli/packages/core/src/tool/handlers/websearch-results.ts";

const links = Array.from({ length: 25 }, (_, i) => `- [Doc ${i}](https://example.com/${i})`).join(
  "\n",
);
const texts = [
  "",
  "   ",
  "Plain answer without links.",
  "See [Rust](https://www.rust-lang.org) and [Docs](https://doc.rust-lang.org/).",
  "Dup [A](https://A.example/x) and [a](https://a.example/x) again.",
  "Image ![logo](https://img.example/logo.png) then [site](https://site.example).",
  "Non-http [local](file:///tmp/x) and [ftp](ftp://x) and [ok](http://ok.example).",
  "Multi\nline [one](https://one.example)\n\n[two](https://two.example/path?q=1)",
  `Many links:\n${links}`,
  "  [spaced title ](https://spaced.example)  ",
  "[](https://empty-title.example)",
];
const outputs = texts.map((text) => {
  const output = buildWebSearchOutput(
    { query: "rust async" },
    { text, finishReason: "stop", usage: {} },
    Date.now(),
  );
  delete output.durationMs;
  return { text, output, content: formatWebSearchModelContent({ ...output, durationMs: 1 }) };
});
const content = `${JSON.stringify(
  {
    // 月份按本地时间取；固定日期在所有时区都落在 2026 年 9 月中旬。
    description: buildWebSearchProviderDescription(new Date(2026, 8, 15, 12)),
    schema: WebSearchInputJsonSchema,
    outputs,
  },
  null,
  1,
)}\n`;
const target = new URL(
  "../apps/zcode-cli-rust/crates/domain/tests/fixtures/websearch_corpus.json",
  import.meta.url,
);
if (process.argv.includes("--check")) {
  const current = await readFile(target, "utf8").catch(() => "");
  if (current !== content) throw new Error("Rust WebSearch corpus differs from TS");
} else await writeFile(target, content);
