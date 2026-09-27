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
  - 用户决定只修工具定义链路：stdio MCP 已按声明顺序发给模型，见 rust-tool-schema-order.md；
    SSE 与 streamable HTTP（SSE 应答）也已覆盖；streamable HTTP 以 JSON 直接应答的 server 与 MCP 工具校验问题的顺序仍按排序。

## 内置工具（2026-09-27）

- 差分探测（Read/Edit/Write/Bash/TodoWrite/WebFetch/Skill 的缺参、类型错误、多余参数）结论：
  - Node 的模型文案都来自同一 JSON Schema 格式化，即发给模型的工具定义里的 `parameters`；
  - Rust 用各工具自己的 `Tool failed: …` 文案。
- 规则：
  - 所有工具，含内置与会话侧工具，在权限请求之前按发给模型的定义 `parameters` 校验，失败文案同 MCP；
  - 未知键的处理取决于 TS runtime schema（zod）是否 strict，由 zod 实测得出：
    - 以下内置工具丢弃未知键：
      - 顶层：Agent、Edit、ExitPlanMode、Glob、Grep、Read、Skill、WebFetch、Write；
      - 嵌套：TodoWrite 的 `todos/*`（逐个遍历 zod v3 schema 实测）；
      - "未知"以 runtime schema 为准，不以发给模型的定义为准：Read 的 runtime schema 总含 `pages`，
        模型不支持 PDF 时发给模型的定义里没有它；此时 `pages` 不被丢弃，按定义校验报"unexpected parameter"
        （差分发现：之前 Rust 丢弃 `pages` 后照常读取）；
      - 去掉未知键后校验通过，则按去掉后的参数执行；
      - 否则按原始参数报告问题，多余参数也一并列出（TS 投影保留 unrecognized_keys）；
    - 其余内置工具（strict）与 MCP 工具（无 runtime schema）按原始参数校验，参数原样传递。
  - 默认值：内置工具缺失且 schema 声明了 `default` 的属性，先按默认值补齐再校验（TS zod `.default()`；
    问题投影也不报有默认值的缺失属性）。例如 AskUserQuestion 的 `multiSelect` 标为必填、默认 false。
    MCP 工具不补。
  - 属性顺序：内置工具 schema（`tool_schemas.json`）按声明顺序登记到 `domain::schema_order`（内容寻址缓存），
    校验时用 `schema_order::ordered` 取声明顺序，问题顺序与发给模型的定义都与 Node 一致。
  - 定义中没有该工具时（如 ToolSearch 尚未加载的延迟工具）不校验。
- 宽松写法（TS runtime schema 的 preprocess/transform，按 contracts 源码逐项移植）：
  - Bash `timeout` 的数字字符串转数字；
  - Bash `run_in_background`/`dangerouslyDisableSandbox` 与 Edit `replace_all` 的
    true/1/yes/y/on、false/0/no/n/off（及数字 1/0）转布尔；
  - TaskOutput `block` 的 "true"/"false"；
  - Skill 旧写法 `{name, args}` 转 `{skill, args}`。
  - 转换先于丢弃未知键与 JSON 校验，执行使用转换后的参数；只用于内置工具。差分见 builtin-results。
- 只在 runtime schema 中存在的细化约束（refine），2026-09-27 差分逐项对齐：
  - Cron：handler 抛出的 ZodError 文案，见 rust-cron.md「refine 文案」；
  - AskUserQuestion：交互 broker 拒绝，见 rust-user-questions.md「入参 refine 失败」；
  - ExitPlanMode：非 plan 模式的策略拒绝先于入参，见 rust-permission-modes.md；
  - WebSearch：先判模型能力，再在 handler 内执行 refine（TS 缺陷已修复），见 rust-websearch.md；
  - Read：`pages` 按 runtime schema 视为已知键（见上文）。
  - 未逐项差分的：工作流类工具（Rust 未实现）与 Read 的 `validateReadInputSemantics`（PDF pages 语义）。
