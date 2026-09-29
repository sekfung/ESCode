// Run with node --import tsx. 已保存工作流文件编解码的 TS oracle（docs/specs/rust-dynamic-workflow.md 第 2 期）：
// 真实 serializeSavedWorkflow / parseSavedWorkflow。序列化逐字节比对；解析比对结果、元数据、脚本与行偏移，
// invalid_yaml 只比对原因（YAML 解析器的错误措辞不同，见规格）。--check 防漂移。
import { readFile, writeFile } from "node:fs/promises";
import {
  parseSavedWorkflow,
  serializeSavedWorkflow,
} from "../apps/zcode-cli/packages/core/src/tool/handlers/saved-workflows/frontmatter.ts";

const long =
  "Review the pull request end to end, checking correctness, test coverage, naming, and whether the change matches the linked issue description in detail.";
const serializeCases = [
  { description: "Summarize the diff" },
  { description: "Triage", whenToUse: "When a new issue arrives" },
  { description: long, whenToUse: long + " " + long },
  { description: "line one\nline two\n" },
  { description: "line one\nline two" },
  { description: "has: colon and # hash" },
  { description: "- leading dash" },
  { description: "yes" },
  { description: "123" },
  { description: "true" },
  { description: "null" },
  { description: "  padded  " },
  { description: 'quote\'s "double"' },
  { description: "中文描述：审查代码" },
  { description: "tab\there" },
  { description: "@at start" },
  { description: "ends with colon:" },
  {
    description: "Args",
    args: {
      pr: { type: "string", required: true, description: "PR number or URL" },
      depth: { type: "number", default: 3 },
      dry: { type: "boolean", default: false },
      extra: { type: "json", default: { a: [1, 2, { b: "c" }], d: null, e: 1.5, f: -2 } },
      list: { type: "json", default: ["x", "y z", ""] },
      empty: { type: "json", default: {} },
      nothing: { type: "json", default: [] },
      str: { type: "string", default: "multi\nline" },
      big: { type: "number", default: 1e21 },
      tiny: { type: "number", default: 0.000001 },
    },
  },
];
// 折行边界：逐个长度生成，覆盖首行（带键前缀）与续行恰好 80 列的情况。
for (let n = 60; n <= 100; n += 1) {
  const words = [];
  let text = "";
  for (let i = 0; text.length < n; i += 1) {
    words.push(["alpha", "be", "gamma", "d", "epsilonzeta", "eta"][i % 6]);
    text = words.join(" ");
  }
  serializeCases.push({ description: text + " " + text + " " + text });
}
serializeCases.push(
  {
    description: "Nested long strings",
    args: {
      target: {
        type: "string",
        description:
          "The repository path or URL to analyze, including the branch name if it differs from the default branch of the project",
        default:
          "https://example.com/org/repository-name/tree/feature/some-long-branch-name-here/path",
      },
    },
  },
  { description: "short\n" + "x".repeat(120) + "\nend" },
  { description: "line one\n" + "word ".repeat(30) + "\nend" },
  { description: "a: " + "long ".repeat(30) },
  {
    description: "List",
    args: {
      l: { type: "json", default: ["word ".repeat(25).trim(), { k: "word ".repeat(25).trim() }] },
    },
  },
  {
    description: "Numbers",
    args: {
      n: {
        type: "json",
        default: [0.1, 100, -0, 1e-7, 123456789012, 2.5e-8, 1e21, 999999999999999999999],
      },
    },
  },
  { description: "Keys", args: { "my-arg.v2": { type: "string" }, 123: { type: "number" } } },
);
const serialized = serializeCases.map((meta) => ({
  meta,
  file: serializeSavedWorkflow(meta, "export default async () => {\n  return 1;\n}\n"),
}));

const script = "const x = 1;\n/* not frontmatter */\nexport default x;";
const parseSources = [
  ...serialized.map((c) => c.file),
  `\n\n/* zcode-workflow\ndescription: Hi\n*/\n${script}`,
  `/* zcode-workflow\r\ndescription: CRLF\r\n*/\r\n${script}`,
  `/* zcode-workflow\ndescription: No trailing newline\n*/`,
  `/* zcode-workflow\ndescription: yes\nwhenToUse: 0x10\n*/\n`,
  `// not a workflow\n${script}`,
  "",
  `/* zcode-workflow\ndescription: never closed\n${script}`,
  `/* zcode-workflow\ndescription: [unclosed\n*/\n`,
  `/* zcode-workflow\ndescription: ""\n*/\n`,
  `/* zcode-workflow\ndescripton: typo\n*/\n`,
  `/* zcode-workflow\ndescription: ok\nargs:\n  a: { type: int }\n*/\n`,
  `/* zcode-workflow\ndescription: ok\nargs:\n  a: { type: string, required: "yes" }\n*/\n`,
  `/* zcode-workflow\ndescription: ok\nargs:\n  a: { type: string, extra: 1 }\n*/\n`,
  `/* zcode-workflow\ndescription: 5\n*/\n`,
  `/* zcode-workflow\n- a list\n*/\n`,
  `/* zcode-workflow\n*/\n`,
  `  /* zcode-workflow  \ndescription: indented sentinel\n  */  \nbody`,
];
const parsed = parseSources.map((source) => ({ source, result: parseSavedWorkflow(source) }));

// `JSON.stringify(-0)` 在 JS 里会写成 `0`，而 yaml `stringifyNumber` 对负零写 `-0`。语料要能重放
// 这个用例，就把负零换成哨兵对象再改写成 `-0.0`（合法 JSON，serde_json 解析为 f64 负零）。
const content = `${JSON.stringify({ serialized, parsed }, (key, value) =>
  Object.is(value, -0) ? { __zcodeNegZero: true } : value,
).replaceAll('{"__zcodeNegZero":true}', "-0.0")}\n`;
const target = new URL(
  "../apps/zcode-cli-rust/crates/domain/tests/fixtures/saved_workflow_corpus.json",
  import.meta.url,
);
if (process.argv.includes("--check")) {
  if ((await readFile(target, "utf8")) !== content)
    throw new Error("Rust saved workflow corpus differs from TS");
} else await writeFile(target, content);
