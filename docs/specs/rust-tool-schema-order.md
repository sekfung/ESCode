# Rust MCP 工具定义的属性顺序

## 背景

rust-tool-input-validation.md 的差分发现：Node 发给模型的 MCP 工具定义保留 inputSchema 的声明顺序，
Rust 按键排序。例如 Node 为 `text, mode, count`，Rust 为 `count, mode, text`。

排序发生在两处：

- rmcp 把 `tools/list` 应答反序列化为 `serde_json::Map`（未开 `preserve_order`）；
- 模型请求体由 `serde_json::Value` 序列化。

JSON 语义相同，但属性顺序对模型可见。用户决定：只修工具定义链路，不全局开启 `preserve_order`（2026-09-27）。

## 规则

1. 捕获：
   - MCP stdio 输出流经旁路读取（tee）。含 `result.tools` 数组的 JSON-RPC 应答行按 id 暂存原文，单行上限与
     codec 相同，为 8 MiB；
   - `tools/list` 请求完成时取出本 id 的原文，按原文顺序得到每个工具的 inputSchema。
2. 登记：
   - 按内容寻址登记：键为该 schema 经 `serde_json` 排序后的紧凑文本，值为按原文顺序、JS `JSON.stringify` 规则的紧凑文本；
   - 相同内容重复登记幂等。
   - 所有者：`host::schema_order`（进程级缓存，只存 schema 文本，无会话语义）。
3. 编码：
   - 模型请求体序列化时（`provider.rs` 唯一编码点），逐个工具取其 schema：
     - chat 为 `function.parameters`；
     - responses 为 `parameters`；
     - anthropic 为 `input_schema`；
   - 查到登记时，把该工具序列化文本中的排序 schema 原位换成保序文本；查不到则保持现状（排序）。
   - 其余 JSON 仍由 `serde_json` 序列化。
   - 纯函数：`domain::schema_order::splice`。
4. SSE 传输：事件数据在 Rust 自有的 SSE 解析处（`mcp_sse.rs`）同样交给捕获表，规则与 stdio 相同。
5. 暂不覆盖（保持排序）：
   - streamable HTTP 传输：rmcp 的 HTTP 客户端只交出解析后的消息，reqwest 也没有响应体中间件；
     要取得原文只能重写 rmcp 的 HTTP 应答处理，暂不做；
   - 内置工具定义，这些本来就由 Rust 内置 JSON 生成，与 Node 的差异另行差分确认。

## 验收

- 单测：
  - 保序文本的数值与转义格式与 JS 一致；
  - 工具中 schema 能原位替换；查不到登记时字节不变。
- App 差分：非字母序声明的 stdio 与 SSE MCP 工具，两侧模型请求里的该工具定义序列化后逐字一致
  （比较 `parameters` 的 `JSON.stringify` 结果）。
