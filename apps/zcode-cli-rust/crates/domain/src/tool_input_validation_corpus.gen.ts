// 生成 tool_input_validation_corpus.json 的 TS oracle 脚本（docs/specs/rust-tool-input-validation.md）：
// node --import tsx apps/zcode-cli-rust/crates/domain/src/tool_input_validation_corpus.gen.ts
import { writeFileSync } from "node:fs";
import { validateJsonSchemaValue } from "../../../../zcode-cli/packages/core/src/tool/json-schema.ts";
import { createInitialInputValidationModelContent } from "../../../../zcode-cli/packages/core/src/tool/input-validation-model-content.ts";

const schema = {
  type: "object",
  properties: {
    title: { type: "string", minLength: 2, maxLength: 5 },
    code: { type: "string" },
    count: { type: "integer", minimum: 1, maximum: 3 },
    ratio: { type: "number" },
    mode: { type: "string", enum: ["fast", "slow"] },
    kind: { const: "tab" },
    tags: { type: "array", items: { type: "string" }, minItems: 1, maxItems: 2 },
    nested: {
      type: "object",
      properties: { id: { type: "string" } },
      required: ["id"],
      additionalProperties: false,
    },
    either: { oneOf: [{ type: "string" }, { type: "number" }] },
    multi: { type: ["string", "null"] },
    loose: {},
  },
  required: ["title", "code", "mode", "ghost"],
  additionalProperties: false,
};
const inputs: unknown[] = [
  { title: "ok", code: "x", mode: "fast", ghost: 1 },
  {},
  { title: "x", code: 1, mode: "medium", extra: true, another: 2 },
  { title: "toolong", code: "c", mode: "slow", ghost: 0, count: 1.5 },
  { title: "ab", code: "c", mode: "slow", ghost: 0, count: 9, ratio: "1" },
  { title: "ab", code: "c", mode: "slow", ghost: 0, kind: "window", tags: [] },
  { title: "ab", code: "c", mode: "slow", ghost: 0, tags: [1, "a", "b"] },
  { title: "ab", code: "c", mode: "slow", ghost: 0, nested: { other: 1 } },
  { title: "ab", code: "c", mode: "slow", ghost: 0, either: true, multi: 3 },
  { title: "ab", code: "c", mode: "slow", ghost: 0, count: 0 },
  [],
  "text",
];
const loose = { ...schema, required: ["title"], additionalProperties: undefined };
const looseInputs: unknown[] = [
  { title: "x" },
  { title: "ab", mode: "medium", kind: "window" },
  { title: "ab", count: 0, tags: [] },
  { title: "ab", tags: ["a", "b", "c"], either: true },
  { title: "ab", nested: {}, multi: 3 },
  { title: "ab", count: 5, ratio: 1e21 },
];
const run = (s: Record<string, unknown>, list: unknown[]) =>
  list.map((input) => {
    const { issues } = validateJsonSchemaValue(input, s as never);
    return {
      input,
      issues,
      content: issues.length
        ? createInitialInputValidationModelContent(
            { metadata: { name: "mcp__demo__run" }, inputSchema: s } as never,
            issues,
            undefined,
          )
        : null,
    };
  });
writeFileSync(
  new URL("./tool_input_validation_corpus.json", import.meta.url),
  JSON.stringify(
    [
      { schema, cases: run(schema, inputs) },
      { schema: loose, cases: run(loose, looseInputs) },
    ],
    null,
    2,
  ) + "\n",
);
