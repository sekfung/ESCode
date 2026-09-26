# Rust 工具入参 schema 校验

2026-09-26。node_repl 差分中发现：Node 在执行工具前按工具 `inputSchema` 校验模型给出的参数。
缺少必填参数时，Node 向模型返回
`<tool_use_error>InputValidationError: <tool> failed due to the following issue:
The required parameter `x` is missing</tool_use_error>`，
且不调用工具。Rust 没有这一层，参数原样交给工具（MCP server 可能接受，也可能以自己的文案报错）。

TS oracle：`core/src/tool/{json-schema,tool-input-validation-issues,input-validation-model-content,input-normalization}.ts`
与 `executor/validation.ts`，合计约 700 行。

## 实现（MCP 工具，已完成）

- 所有者：`domain::tool_input_validation`（纯函数）；`core::tool_dispatch` 在 ToolStart 之后、权限请求之前调用。
- 规则：
  - 逐项移植 `validateJsonSchemaValue`：oneOf、const、enum、type、字符串与数值上下限、数组、对象、required、
    `additionalProperties: false`；
  - 问题对象的字段顺序与 TS 相同，JSON 回落文案用保序的 `json_order::Json`；
  - 模型可见文案为 `<tool_use_error>InputValidationError: …</tool_use_error>`：
    - 缺失、多余、类型错误三类合并成若干行；
    - 其余问题回落为 `JSON.stringify(issues, null, 2)`。
- 失败时的处理：
  - 工具结果标记为失败，不请求权限，不调用 MCP server；
  - 入参按原文解析，保留键顺序，多余参数按模型给出的顺序列出。
- 验收：
  - TS oracle 语料 `tool_input_validation_corpus.json` 共 18 例，问题对象与文案逐字一致；
    生成脚本为同目录的 `.gen.ts`；
  - App 差分 `zcode-cli-rust-tool-input-validation.test.ts`：缺参、多余参数、类型错误、枚举与下限、合法调用，
    六类模型文案逐字一致，且只有合法调用到达 MCP server。

## 已知差异（待与产品对齐）

- Rust 的 JSON 值（`serde_json` 未开 `preserve_order`）按键排序：
  - rmcp 返回的 MCP inputSchema 在进入 Rust 前已排序，因此属性声明顺序丢失；
  - 发给模型的 MCP 工具定义，包括 `function`/`parameters` 各层，与 Node 的键顺序不同。差分实测：Node 保留
    `text, mode, count` 的声明顺序，Rust 为 `count, mode, text`。
  - 影响：
    - 模型看到的属性顺序不同；
    - 多个问题同属一类时，校验问题的顺序可能不同。
  - 修复需要全局开启 `preserve_order`，或让工具定义链路改用保序 JSON；涉及面广，单独决定。
- 内置工具：TS 另有 runtime schema（zod）问题与 JSON 问题的投影合并；Rust 内置工具保持现有校验文案，
  待差分确认后再定范围。
