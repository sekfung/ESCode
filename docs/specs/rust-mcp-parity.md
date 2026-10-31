# Rust MCP 对齐：结果格式、图片与 OAuth

2026-09-26。承接 rust-media-read.md 第 5 期与用户选定的「MCP OAuth」缺口。本文先记录差距与分期，实施前逐期补充规则与验收。

## 现状差距（按 TS `core/src/mcp/index.ts` `formatMcpToolResult` 与 `adapters/src/mcp/*` 对照）

| 项                     | TS                                                                                             | Rust 现状                                                                                                                                                                      |
| ---------------------- | ---------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 文本块拼接             | 跳过空文本，以 `\n\n` 连接；全是文本时收成字符串                                               | 以 `\n` 连接，不跳过空文本                                                                                                                                                     |
| structuredContent      | 有内容时追加 `Structured content:\n<两空格缩进 JSON>`                                          | 追加紧凑 JSON                                                                                                                                                                  |
| isError                | `MCP tool returned an error:\n<文本>`（server 声明 message-only 时除外）                       | 无前缀                                                                                                                                                                         |
| image                  | 转为 image 块（占位名 `MCP image`），并经 `image-normalization` 做 inline 预算（200KiB）与压缩 | 已对齐（第 2 期）                                                                                                                                                              |
| audio                  | `[MCP audio content omitted: <mime>]`                                                          | 整块 JSON 文本                                                                                                                                                                 |
| resource               | `MCP resource content:\n<JSON>`                                                                | 整块 JSON 文本                                                                                                                                                                 |
| 截图 artifact / CUA 帧 | 浏览器截图落 artifact；官方 CUA 帧走 integrity 通道                                            | 无                                                                                                                                                                             |
| OAuth                  | 交互式授权（浏览器回调）、凭据存储、刷新、租约、官方账号授权（约 2200 行）                     | 授权码 + PKCE 与 client_credentials 已实现并与 Node 差分一致（rust-mcp-oauth.md）；官方 MCP 的 http 身份头与 stdio `_meta` 载荷已与 Node 差分一致（rust-mcp-official-auth.md） |

## 分期

1. （已完成）结果格式对齐：文本拼接、structuredContent、isError 前缀、audio/resource 文案；通过 MCP 差分覆盖（`escode-cli-rust-mcp-result-differential.test.ts`，Chat 与 Anthropic 两种协议）。
   - 缩进 JSON 按 rmcp 类型字段的顺序输出，resource 与 Node 一致。
   - `structuredContent` 在 rmcp 中是无序 Value，多键时键序可能与 server 原序不同（已知差异）。
2. （已完成）图片结果：
   - 走 `ToolOutput.media`，与 rust-media-read.md 第 1 期的通道相同，投影规则见该文；
   - 超过 200KiB inline 预算：与 App 中的 TS（有 artifact store）相同，写二进制 artifact
     `<artifacts>/<session>/<toolCallId>-tool-result-<uuid><ext>`（扩展名按 TS `extensionForMimeType`，未知为 `.bin`），
     告知模型 `MCP image content saved instead of being inlined: …
Artifact: <path>
Artifact URI: escode-artifact://<session>/tool-result-<uuid>`；
     tool call id 经 `ToolPort::execute_mcp` 传入。artifact 根目录是 Rust 工具产物目录（与 Bash 输出文件相同），
     与 TS 的 `<storageRoot>/cli/artifacts` 不同址——两侧工具产物路径本就各自独立，文件名与 URI 格式一致；
   - 写 artifact 失败使工具调用失败（TS 同样抛错）；未提供 call id 的内部调用仍给出「无 artifact store」说明；
   - node_repl 截图压缩属于浏览器工具，Rust 未提供该工具，归入第 4 期；
   - 差分：`escode-cli-rust-mcp-result-differential.test.ts` 的 `big` 形态核对两侧文案、文件名格式与落盘字节。
3. （已完成，授权码 + PKCE）OAuth：见 rust-mcp-oauth.md。
4. 截图 artifact 与 CUA 帧：依赖官方插件运行时，单独评估。

## 协议协商默认值（2026-09-26）

OAuth 差分中发现：未写 `protocolVersion` 时，两个 runtime 的默认行为不同。
TS `resolveVersionNegotiationMode` 的规则：

| 配置                         | TS            | Rust（修复前）  | Rust（修复后）                                |
| ---------------------------- | ------------- | --------------- | --------------------------------------------- |
| `2026-07-28`                 | pin（无回落） | Discover        | Discover                                      |
| SSE（其余取值）              | legacy        | 按取值          | legacy `initialize`                           |
| `legacy`                     | legacy        | legacy          | legacy                                        |
| 未写 / `auto`（http、stdio） | auto          | 未写时为 legacy | Auto（先 `server/discover`，失败回落 legacy） |

- 已知差异：rmcp 的 Auto 探测超时固定为 10s，TS 为 min(5s, timeout/2)。
  只有 server 对 `server/discover` 完全不响应时，两边才会表现不同；两边超时后都回落 legacy。
- 验收：
  - 在未写 `protocolVersion` 的 HTTP fixture 上，Node 与 Rust 的请求方法序列一致（先 `server/discover`，再 `initialize`）；
  - SSE 的 `auto` 不发 `server/discover`；
  - 现有 MCP 用例不回归。

## SSE 应答的 `server/discover` 错误（2026-09-27）

schema 顺序差分发现：legacy streamable HTTP server 以 SSE（`text/event-stream`）应答 POST，
对 `server/discover` 回 JSON-RPC 错误（如 -32601）时，两边的行为不同：

- Node 把该错误判为 legacy，回落 `initialize`，连接正常；
- rmcp 在启动阶段读 SSE 应答时（`expect_initialized`）只接受 `Response`，会跳过 `Error`，
  随后以 "empty sse stream" 结束 worker。连接失败，该 server 的工具不出现。

修复规则：

- 所有 HTTP client 外面的统一包装层（`mcp_http_capture.rs`）处理 `server/discover` 的 SSE 应答：
  读到第一条 `Response` 或 `Error` 即转为 JSON 应答交给 rmcp（保留 session id），由 rmcp 的 Auto 协商判为 legacy 后回落；
- 其它请求的 SSE 应答不变；流结束仍未读到应答时，按 rmcp 原语义报 "empty sse stream"。

验收：以 SSE 应答、对 `server/discover` 回 -32601 的 HTTP fixture，两侧都回落 `initialize` 并把工具发给模型
（`escode-cli-rust-tool-schema-order.test.ts` 的 http 用例）。

## legacy SSE 自动重连（2026-10-31）

TS 侧 `SSEClientTransport` 使用 `eventsource`：GET 流结束或网络错误后按服务端 `retry:`（缺省 3000ms）
重连；重连后 server 可重新下发 `endpoint`，POST 端点随之更新。`eventsource` 本身会携带 `Last-Event-ID`，
但当前 `@modelcontextprotocol/sdk`（1.29）的 SSE transport 在自定义 `fetch` 里每次用 `_commonHeaders()` 重建 Headers，覆盖了该值，
所以 Node 实际发出的重连 GET 没有 `Last-Event-ID`（`apps/escode-cli/packages/cli/dist/escode.cjs` 可证）。
Rust 按 EventSource 语义携带已见 id，是记录在案的有意差异：对 server 恢复更完整，客户端行为不变。
HTTP 非 2xx、非 `text/event-stream` 或 204 是终止错误，不再重连；POST 请求本身绝不自动重试
（MCP 调用可能改变外部状态）。

Rust 规则（`crates/tools/src/mcp_sse.rs`）：

- 首次 GET 失败、非 SSE 响应或 204 时 `open()` 失败；首次连接在收到 `endpoint` 前结束则按同一间隔继续重连。
- 流结束或网络错误后重连同一 URL；解析 `id:` 并在后续 GET 携带 `Last-Event-ID`，空 `id:` 清除该值。
- 解析 `retry:` 毫秒值（非法值忽略；下限 1ms、上限 300s 防止忙等/超长挂起）；缺省 3000ms。
- 重连收到 `endpoint` 时校验同源并更新 POST 端点；未收到则沿用旧端点。
- 重连 HTTP 非 2xx、非 `text/event-stream` 或 204 终止（通道关闭，rmcp 报连接结束）；POST 失败不重试。
- 取消/关闭令牌打断等待与流读取；取消后不得再重连。

验收：

- Rust 单测覆盖 `id:`/`retry:`/多块分帧，以及重连后的 `Last-Event-ID`、endpoint 更新与 POST 不重试。
- Node/Rust 差分：fixture 在 `initialize` 后主动断开 SSE，随后 `tools/list` 仍成功，两侧 GET 次数相同；
  `Last-Event-ID` 记录为 Node `[null, null]`（SDK 覆盖 header）、Rust `[null, "1"]`（已见 id）。

## 大结果预算（2026-10-31）

Node 普通 MCP 工具使用通用 `resultBudget`（`apps/escode-cli/packages/core/src/mcp/index.ts`）：
`maxInlineBytes=100_000`、`maxModelBytes=50_000`、`strategy=truncate`、`preview=head`。
超过 50_000 字节的模型文本保留开头，并追加：

```text

[Tool output truncated by resultBudget: originalBytes=<n>, maxModelBytes=50000, strategy=truncate]
```

Rust 旧实现到 64 KiB 才截断，且只追加 `[MCP result truncated]`，与 Node 不一致。现规则
（`crates/tools/src/mcp_connection.rs`）：

- 仅处理格式化后的模型文本 `formatted.text`；图片等 media block 继续走各自的 artifact/inline 预算。
- `bytes <= 50_000` 原样返回；`bytes > 50_000` 先为 suffix 留位，再按 UTF-8 字符边界截 head。
- 截断后总字节数不超过 50_000，suffix 的 `originalBytes` 使用截断前 UTF-8 字节数。
- `node_repl` 的 `js` 使用 artifact 预算（Node `maxInlineBytes=1_000_000`、`maxModelBytes=64 KiB`、
  `strategy=artifact`、`retention=session`、`preview=tail`）：超过 64 KiB 时把完整模型文本写入
  `<root>/<session>/<toolCallId>-tool-result-<uuid>.json`，并向模型返回通用 `<persisted-output>` 信封
  （十进制字节、2 000 字符预览、换行截断规则与 Node `formatPersistedOutputEnvelope` 一致）。
  artifact 写入失败时回退为 tail 截断，suffix 的 `strategy=artifact` 与 Node 相同，不让一次成功的工具调用失败。
- 官方 CUA server 只有在 `official_authority_verified` 成立时才使用 Node 的 256 KiB `truncate` 预算
  （`maxInlineBytes=maxModelBytes=256*1024`、`preview=head`）：suffix 与普通 MCP 相同，`maxModelBytes=262144`，
  截断后总字节数不超过 256 KiB。成立条件对齐 TS `computeOfficialCuaServerNames`：非 `node_repl`、stdio、
  env `ESCODE_PLUGIN_ID` 去空白并小写后等于 `computer-use@zcode-plugins-official`，且 env
  `ESCODE_CUA_PLUGIN_AUTHORITY` 去空白后等于宿主启动时捕获的 authority；socket 与 authority 必须成组，
  缺失或半组 fail-closed。server 名、tool 名和 manifest env 都不单独构成信任。条件不成立时保留既有
  64 KiB `[MCP result truncated]` 行为，不按名字放宽。
- 当前产品中除 `node_repl` 外的 CUA 形态 stdio server 在连接前已被 `is_retired` 退役（Node 与 Rust 相同），
  因此 256 KiB 分支只在未来官方 CUA MCP server 恢复时可达；`node_repl` 不受该分支影响，继续走 artifact 预算。

验收：`packages/services/tests/escode-cli-rust-mcp-large-result-differential.test.ts` 对
OpenAI Chat 与 Anthropic Messages 两种投影比较 50_000 边界、60_000 ASCII 与 60_000 字节中文；
`packages/services/tests/escode-cli-rust-node-repl.test.ts` 用真实内置 `node_repl` 比较 >64 KiB `js` 输出
（artifact 路径归一化后信封逐字一致）。Node 观察冻结为仓内 golden；Rust 单测覆盖边界、suffix、UTF-8
字符边界、artifact 信封、特殊 server 的保留行为，以及 authority 命中、缺失、半组、错误 authority、非 stdio
和 `node_repl` 例外。

## 配置内凭据变更（2026-10-31）

设置页 `mcp/list` 的进程级连接与 session 级 MCP 配置都按完整 server 配置（含 `env` / `headers`）区分连接身份。
同一 server 名下的凭据字段变化后，下一次连接必须使用新值，不能命中旧连接：

- Node `MCPConnectionPool.acquire` 的 `connectionKey` 为 `serverName + isolation scope + stableStringify(config)`
  （`apps/escode-cli/packages/adapters/src/mcp/pool.ts`）；config 变化即创建新 entry，旧 entry 在释放 lease 后
  按 idle grace 关闭。
- Rust `Server::key()` 包含 `name` / `cwd` / 完整 `raw`（`crates/plugins/src/mcp_config.rs`）；`prepare_inner`
  发现同名新 key 后建立新连接，`prune()` 立即关闭不再被引用的旧连接。
- 两侧的共同可观察结果：`mcp/list` 第二次返回 `connected`，新连接的 initialize 使用新凭据；随后用新配置
  创建的 session 调用工具，工具看到的是新凭据。旧连接关闭时机不同（Node 有 idle grace，Rust 立即）属于实现细节，
  不比较 PID 存活或关闭时间。

验收：`packages/services/tests/escode-cli-rust-mcp.test.ts` 的「MCP credential change reconnects with the
rotated env」用例，在同一 fixture 上把 `MCP_PROBE` 从 `scoped` 改为 `rotated`，比较两侧的 `mcp/list` 状态、
initialize 记录的凭据序列与工具返回的 `probe`；「MCP HTTP credential change reconnects with the rotated
header」用例同样把 `X-Fixture` 从 `configured` 改为 `rotated`，比较两侧状态与 server 实际收到的 header 集合。

## 关闭语义（2026-09-26）

- Node（SDK 2.0 `transport.close`）关闭 Streamable HTTP 连接时不终止会话，不发送 `DELETE`。
- rmcp 的 reqwest client 默认在关闭时发送 `DELETE`。
- Rust 的所有 HTTP 连接（无鉴权、OAuth、client_credentials、官方）都经自有 client，其 `delete_session` 为空操作，与 Node 一致。
- 验收：`escode-cli-rust-mcp-negotiation-differential.test.ts` 比较服务端收到的 `DELETE` 次数（两侧均为 0）。
