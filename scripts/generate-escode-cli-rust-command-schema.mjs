import { readFile, writeFile } from "node:fs/promises";

// `v4/command` 的 schema 语料：从 TS zod 生成（rust-unified-host.md P1 #10）。
// 单一来源是 TS（packages/shared/src/escode-protocol-v4/command.ts），Rust 侧
// `crates/protocol/src/command_schema.rs` 用同一份 JSON 校验命令信封与 payload，
// 漂移由 `--check` 与 Rust 侧漂移测试双向拦截。
const source = new URL(
  "../packages/shared/src/escode-protocol-v4/command.ts",
  import.meta.url,
);
const { z } = await import("zod");
const {
  commandEnvelopeSchema,
  commandPayloadSchemas,
  commandTypeSchema,
  COMMANDS_REQUIRING_BASE_REVISION,
  ROW_TARGETING_COMMANDS,
} = await import(source.href);

// io: "input" 与 TS 的解析方向一致；unrepresentable: "any" 让无法静态表达的分支
// （自定义 refine / 惰性联合）退化为宽松节点而不是抛错——Rust 侧对这些节点不做判定。
const options = { io: "input", unrepresentable: "any" };
const payloads = {};
for (const [type, schema] of Object.entries(commandPayloadSchemas)) {
  payloads[type] = z.toJSONSchema(schema, options);
}
const data = {
  note: "Generated from packages/shared/src/escode-protocol-v4/command.ts; do not edit by hand.",
  envelope: z.toJSONSchema(commandEnvelopeSchema, options),
  payloads,
  types: commandTypeSchema.options,
  requiringBaseRevision: [...COMMANDS_REQUIRING_BASE_REVISION].sort(),
  rowTargeting: [...ROW_TARGETING_COMMANDS].sort(),
};
const content = `${JSON.stringify(data, null, 2)}\n`;
const target = new URL(
  "../apps/escode-cli-rust/crates/protocol/src/command_schema.json",
  import.meta.url,
);
if (process.argv.includes("--check")) {
  if ((await readFile(target, "utf8")) !== content) {
    throw new Error(
      "Rust command schema differs from TS zod; run node --import tsx scripts/generate-escode-cli-rust-command-schema.mjs",
    );
  }
  console.log("Rust command schema: OK");
} else {
  await writeFile(target, content);
  console.log(`Wrote ${target.pathname} (${content.length} bytes)`);
}
