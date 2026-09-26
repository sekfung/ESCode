# Rust 工具入参 schema 校验

2026-09-26。node_repl 差分中发现：Node 在执行工具前按工具 `inputSchema` 校验模型给出的参数。
缺少必填参数时，Node 向模型返回
`<tool_use_error>InputValidationError: <tool> failed due to the following issue:
The required parameter `x` is missing</tool_use_error>`，
且不调用工具。Rust 没有这一层，参数原样交给工具（MCP server 可能接受，也可能以自己的文案报错）。

TS oracle：`core/src/tool/{json-schema,tool-input-validation-issues,input-validation-model-content,input-normalization}.ts`
与 `executor/validation.ts`，合计约 700 行。

## 范围（待实施）

- MCP 工具：只做 JSON Schema 校验，问题清单与 TS `validateJsonSchemaValue` 一致；
  按 `formatToolInputValidationError` 生成模型可见文案（缺失 / 多余 / 类型错误三类，其余回落 JSON）。
- 内置工具：TS 另有 runtime schema（zod）问题与 JSON 问题的投影合并。需先差分确认 Rust 现有文案与 Node 的差异，再决定范围。
- 验收：TS oracle 语料（校验问题与格式化文案）；App 差分覆盖 MCP 缺参、多余参数、类型错误。
