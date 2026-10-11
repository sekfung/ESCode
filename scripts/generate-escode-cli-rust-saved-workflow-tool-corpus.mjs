// Run with node --import tsx. ListSavedWorkflows 的模型面与行级 display 的 TS oracle
// （docs/specs/rust-dynamic-workflow.md 第 2 期）：真实工具条目 + 真实 display 构造。
// 工具通道的 24 KiB 预算由 executor 施加（`fitContentWithSuffix`），这里记录的是施加前的格式化结果；
// display 通道的 2 KiB 上限在构造器内，语料覆盖它。--check 防漂移。
import { readFile, writeFile } from "node:fs/promises";
import { listSavedWorkflowsToolEntry } from "../apps/escode-cli/packages/core/src/tool/handlers/list-saved-workflows.ts";
import { createWorkflowObservationDisplay } from "../apps/escode-cli/packages/core/src/tool/executor/workflow-observation-display.ts";

const check = process.argv.includes("--check");
const target = new URL(
  "../apps/escode-cli-rust/crates/tools/tests/fixtures/saved_workflow_tool_corpus.json",
  import.meta.url,
);

const entry = (overrides) => ({
  name: "review",
  description: "Review the pull request end to end.",
  scope: "project",
  path: "/project/.escode/workflows/review.dwf.ts",
  ...overrides,
});

const outputs = [
  // 空项目：必须说成一句话，而不是空容器。
  { workflows: [] },
  { workflows: [entry({})] },
  {
    workflows: [
      entry({
        name: "triage",
        description: "Triage a new issue",
        whenToUse: "When a new issue arrives",
        scope: "global",
        path: "/home/.escode/workflows/triage.dwf.ts",
        args: {
          pr: { type: "string", required: true, description: "PR number or URL" },
          depth: { type: "number", default: 3 },
          dry: { type: "boolean", default: false },
          extra: { type: "json", default: { a: [1, 2], b: null } },
          empty: { type: "json", default: {} },
        },
      }),
    ],
  },
  {
    workflows: [entry({ name: "quoted", description: 'a "quoted" <name> & path' })],
    invalid: [
      { path: '/project/.escode/workflows/bad & "name".dwf.ts', reason: "invalid_yaml: bad block" },
    ],
  },
  // display 侧 2 KiB 上限：描述超长时截断并打 truncated。
  { workflows: [entry({ description: "word ".repeat(600).trim() })] },
  // 工具通道 24 KiB 预算（施加前缀的原始格式化结果）。
  {
    workflows: Array.from({ length: 200 }, (_, index) =>
      entry({
        name: `wf-${index}`,
        description: "x".repeat(200),
        path: `/project/.escode/workflows/wf-${index}.dwf.ts`,
      }),
    ),
  },
];

const corpus = outputs.map((output) => ({
  output,
  modelContent: listSavedWorkflowsToolEntry.formatModelContent(output),
  display: createWorkflowObservationDisplay("ListSavedWorkflows", output) ?? null,
}));
const content = `${JSON.stringify(corpus)}\n`;
if (check) {
  if ((await readFile(target, "utf8").catch(() => "")) !== content)
    throw new Error("Rust saved workflow tool corpus differs from TS");
} else await writeFile(target, content);
