// Run with node --import tsx. ListModels 的模型面与行级 display 的 TS oracle
// （docs/specs/rust-dynamic-workflow.md 第 6 期前置）：真实工具条目 + 真实 display 构造。
// 工具通道的 24 KiB 预算由 executor 施加，这里记录施加前的格式化结果（Rust 侧另有同规则的单测）；
// display 通道的 2 KiB 元文本上限与 100 行封顶在构造器内，语料覆盖它们。--check 防漂移。
import { readFile, writeFile } from "node:fs/promises";
import { listModelsToolEntry } from "../apps/escode-cli/packages/core/src/tool/handlers/list-models.ts";
import { createWorkflowObservationDisplay } from "../apps/escode-cli/packages/core/src/tool/executor/workflow-observation-display.ts";

const check = process.argv.includes("--check");
const target = new URL(
  "../apps/escode-cli-rust/crates/domain/tests/fixtures/model_catalog_corpus.json",
  import.meta.url,
);

// 键序与 TS handler 的输出字面量一致：id → providerId → modelId → providerLabel? → reasoningLevels
// → defaultReasoningLevel? → contextWindow? → disabledReason?（首次出现的位置即定序）。
const model = (overrides = {}) => {
  const { providerLabel, ...rest } = overrides;
  return {
    id: "personal:fixture/model-a",
    providerId: "personal:fixture",
    modelId: "model-a",
    ...(providerLabel === undefined ? {} : { providerLabel }),
    reasoningLevels: ["low", "high"],
    ...rest,
  };
};

const outputs = [
  // 一个都没配：必须说成一句话。
  { models: [] },
  { current: "personal:fixture/model-a", models: [model({})] },
  {
    models: [
      model({ providerLabel: "Fixture" }),
      model({
        id: "personal:fixture/model-b",
        modelId: "model-b",
        providerLabel: "Fixture",
        reasoningLevels: [],
      }),
      model({
        id: "account:fixture/model-c",
        providerId: "account:fixture",
        modelId: "model-c",
        reasoningLevels: ["low"],
        defaultReasoningLevel: "low",
        contextWindow: 200000,
      }),
      model({
        id: "personal:fixture/model-d",
        modelId: "model-d",
        reasoningLevels: ["low", "medium", "high"],
        defaultReasoningLevel: "high",
        contextWindow: 128000,
        disabledReason: "no API key configured for this provider",
      }),
    ],
  },
  // display 侧 2 KiB 元文本上限：providerLabel 超长时截断并打 truncated。
  { models: [model({ providerLabel: "L".repeat(3000) })] },
  // display 侧 100 行封顶 + 行内顺序（current 在会话选择上）。
  {
    current: "personal:fixture/model-120",
    models: Array.from({ length: 130 }, (_, index) =>
      model({
        id: `personal:fixture/model-${index}`,
        modelId: `model-${index}`,
        reasoningLevels: ["low", "high"],
        defaultReasoningLevel: "high",
      }),
    ),
  },
];

const corpus = outputs.map((output) => ({
  output,
  modelContent: listModelsToolEntry.formatModelContent(output),
  display: createWorkflowObservationDisplay("ListModels", output) ?? null,
}));
const content = `${JSON.stringify(corpus)}\n`;
if (check) {
  if ((await readFile(target, "utf8").catch(() => "")) !== content)
    throw new Error("Rust model catalog corpus differs from TS");
} else await writeFile(target, content);
