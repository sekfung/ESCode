# Rust WebSearch（provider-native 搜索）

2026-09-26。对齐 TS `core/src/tool/handlers/websearch.ts`、`websearch-results.ts`、
`runtime/methods/config.ts#shouldExposeWebSearch` 与 AI SDK 的 `anthropic.web_search_20260209` 编码。
发布门槛中列为剩余缺口：模型声明 `supportsNativeWebSearch` 时 Node 向主模型暴露 WebSearch，Rust 不暴露。

## 产品规则

- 可见性：只有当前绑定模型的 `properties.supportsNativeWebSearch == true` 时，WebSearch 进入主请求工具面；
  位置按 TS `providerOrder`（在 WebFetch 之后、Write 之前）。
- 定义：schema 取 TS `WebSearchInputJsonSchema`（`query` 至少 2 字符、`allowed_domains` / `blocked_domains`），
  描述随当前本地月份生成（`The current month is <Month YYYY>`，每次构造定义时重新计算）。
- 输入校验：`query` 为至少 2 个字符的字符串；两个域名列表不能同时非空；可选 `maxUses` 为 1–8 的整数（默认 8）。
- 执行：辅助调用（最低推理档位、`maxOutputTokens = min(4096, 模型上限)`、60s 超时），请求
  - `system`: `You are an assistant for performing a web search tool use.`
  - `user`: `Perform a web search for the query: <query>`
  - 唯一工具：provider-native `web_search`，Anthropic 编码为
    `{"type":"web_search_20260209","name":"web_search","max_uses":N,"allowed_domains"?,"blocked_domains"?}`，
    并附 `anthropic-beta: code-execution-web-tools-2026-02-09`（与配置中已有的 beta 逗号合并）。
  - 其他协议不编码 native 搜索：工具调用失败（TS `does not encode provider-native WebSearch`）。
- 流式响应：`server_tool_use`、`web_search_tool_result` 块与 `citations_delta` 不作为客户端工具调用，
  只收集文本；`stop_reason = pause_turn` 视为正常结束。
- 结果（TS 流式收集只保留文本，`toolResults`/`sources` 为空）：
  - `summary` = 文本 trim 后非空时；`sources` = summary 中 markdown 链接 `[title](http…)`（跳过图片 `![…]`），
    按 URL 小写去重；`results` 为空。
  - 模型可见内容：

    ```
    Web search results for query: "<query>"

    Summary:
    <summary>

    Links:
    - [title](url)            （最多 20 条；无链接时 `- No links found.`）

    REMINDER: You MUST include the sources above in your response to the user using markdown hyperlinks.
    ```

    （无 summary 时省略 Summary 段），整体 trim。

  - App 数据：`{query, results, sources, summary?, durationMs, webSearchRequests?}`。

## 已知差异

- App 数据中的 `webSearchRequests`（TS 取自 usage 的 `server_tool_use.web_search_requests`）暂不填写；模型可见结果不受影响。
- 声明了 native 搜索却不是 Anthropic 协议的模型：TS 抛出「does not encode provider-native WebSearch」，Rust 以
  `invalid_model_request` 失败，二者都是工具失败，文案不同。

## 所有者

- 会话侧（core）负责可见性判定与执行编排（与 WebFetch 相同的位置）；provider-native 工具的线上编码与流解析在
  model adapter；搜索文本不进入会话 transcript（隐藏流，只转发重试与鉴权事件）。

## 验收

- 语料：`scripts/generate-zcode-cli-rust-websearch-corpus.mjs` 以 TS `formatWebSearchModelContent`/`buildWebSearchOutput`
  为 oracle 覆盖 summary 链接抽取、去重、20 条上限、无链接与无 summary。
- 附带修复：Anthropic 非缓存请求（标题、WebFetch 处理、WebSearch、压缩摘要）的 `system` 与 AI SDK 一致，
  始终为文本块数组、无 system 时省略（此前拼成字符串）。
- App 差分（`zcode-cli-rust-websearch-differential.test.ts`，已通过）：模型声明 `supportsNativeWebSearch` 的 Anthropic 协议下，Node 与 Rust
  - 主请求工具面包含同样的 WebSearch 定义；
  - 内部搜索请求的 system/user/工具/`max_tokens`/beta 头一致；
  - 响应含 `server_tool_use` / `web_search_tool_result` / `citations_delta` 时，工具结果文本一致；
  - 不支持 native 搜索的模型不暴露 WebSearch。
