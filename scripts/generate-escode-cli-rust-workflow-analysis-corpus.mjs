// Run with node --import tsx. 工作流脚本分析的 TS oracle（docs/specs/rust-dynamic-workflow.md 第 3 期，路线 1）：
// 真实 analyzeWorkflowScript + encodeAnalysisCore，与 CLI 隐藏子命令 `__escode-workflow-analyzer` 的 JSON 形一致。
// Rust 经子进程桥得到的结果逐字比对（crates/tools/src/workflow_analyzer.rs）。--check 防漂移。
// 样例脚本取自内置技能 dynamic-workflows 的示例（真实的模型面写法），另加几条编译 / facade 错误。
import { readFile, writeFile } from "node:fs/promises";
import {
  analyzeWorkflowScript,
  encodeAnalysisCore,
} from "../apps/escode-cli/packages/dynamic-workflow/src/index.ts";

const skillDir = new URL(
  "../apps/escode-cli/packages/bundled-skills/skills/dynamic-workflows/",
  import.meta.url,
);
const blocks = [];
for (const file of ["examples.md", "patterns.md"]) {
  // 检出的行尾随平台不同（Windows autocrlf）：统一成 LF，语料与平台无关。
  const text = (await readFile(new URL(file, skillDir), "utf8")).replace(/\r\n/g, "\n");
  for (const match of text.matchAll(/```(?:ts|typescript|js|javascript)\r?\n([\s\S]*?)```/g)) {
    blocks.push({ name: `${file}#${blocks.length}`, script: match[1] });
  }
}

const cases = [
  ...blocks.slice(0, 8),
  { name: "type-error", script: 'const x: number = "s";\n' },
  { name: "syntax-error", script: "export default async function ( {\n" },
  { name: "empty", script: "" },
  { name: "unicode", script: "// 中文注释\nconst greeting: string = 1;\n" },
];

const analyze = (script) => {
  const { core, ...rest } = analyzeWorkflowScript(script);
  return { ...rest, ...(core ? { core: encodeAnalysisCore(core) } : {}) };
};

const content = `${JSON.stringify({
  cases: cases.map((c) => ({ name: c.name, script: c.script, result: analyze(c.script) })),
})}\n`;
const target = new URL(
  "../apps/escode-cli-rust/crates/tools/src/workflow_analysis_corpus.json",
  import.meta.url,
);
if (process.argv.includes("--check")) {
  if ((await readFile(target, "utf8")) !== content)
    throw new Error("Rust workflow analysis corpus differs from TS");
} else await writeFile(target, content);
