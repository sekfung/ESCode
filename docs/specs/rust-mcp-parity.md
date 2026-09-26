# Rust MCP 对齐：结果格式、图片与 OAuth

2026-09-26。承接 rust-media-read.md 第 5 期与用户选定的「MCP OAuth」缺口。本文先记录差距与分期，实施前逐期补充规则与验收。

## 现状差距（按 TS `core/src/mcp/index.ts` `formatMcpToolResult` 与 `adapters/src/mcp/*` 对照）

| 项                     | TS                                                                                             | Rust 现状                                                                                                            |
| ---------------------- | ---------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------- |
| 文本块拼接             | 跳过空文本，以 `\n\n` 连接；全是文本时收成字符串                                               | 以 `\n` 连接，不跳过空文本                                                                                           |
| structuredContent      | 有内容时追加 `Structured content:\n<两空格缩进 JSON>`                                          | 追加紧凑 JSON                                                                                                        |
| isError                | `MCP tool returned an error:\n<文本>`（server 声明 message-only 时除外）                       | 无前缀                                                                                                               |
| image                  | 转为 image 块（占位名 `MCP image`），并经 `image-normalization` 做 inline 预算（200KiB）与压缩 | 已对齐（第 2 期）                                                                                                    |
| audio                  | `[MCP audio content omitted: <mime>]`                                                          | 整块 JSON 文本                                                                                                       |
| resource               | `MCP resource content:\n<JSON>`                                                                | 整块 JSON 文本                                                                                                       |
| 截图 artifact / CUA 帧 | 浏览器截图落 artifact；官方 CUA 帧走 integrity 通道                                            | 无                                                                                                                   |
| OAuth                  | 交互式授权（浏览器回调）、凭据存储、刷新、租约、官方账号授权（约 2200 行）                     | 授权码 + PKCE 已实现并与 Node 差分一致（rust-mcp-oauth.md）；client_credentials 与官方账号授权仍为 not_authenticated |

## 分期

1. （已完成）结果格式对齐：文本拼接、structuredContent、isError 前缀、audio/resource 文案；通过 MCP 差分覆盖（`zcode-cli-rust-mcp-result-differential.test.ts`，Chat 与 Anthropic 两种协议）。
   - 缩进 JSON 按 rmcp 类型字段的顺序输出，resource 与 Node 一致。
   - `structuredContent` 在 rmcp 中是无序 Value，多键时键序可能与 server 原序不同（已知差异）。
2. （已完成）图片结果：
   - 走 `ToolOutput.media`，与 rust-media-read.md 第 1 期的通道相同，投影规则见该文；
   - 超过 200KiB inline 预算：与 App 中的 TS（有 artifact store）相同，写二进制 artifact
     `<artifacts>/<session>/<toolCallId>-tool-result-<uuid><ext>`（扩展名按 TS `extensionForMimeType`，未知为 `.bin`），
     告知模型 `MCP image content saved instead of being inlined: …
Artifact: <path>
Artifact URI: zcode-artifact://<session>/tool-result-<uuid>`；
     tool call id 经 `ToolPort::execute_mcp` 传入。artifact 根目录是 Rust 工具产物目录（与 Bash 输出文件相同），
     与 TS 的 `<storageRoot>/cli/artifacts` 不同址——两侧工具产物路径本就各自独立，文件名与 URI 格式一致；
   - 写 artifact 失败使工具调用失败（TS 同样抛错）；未提供 call id 的内部调用仍给出「无 artifact store」说明；
   - node_repl 截图压缩属于浏览器工具，Rust 未提供该工具，归入第 4 期；
   - 差分：`zcode-cli-rust-mcp-result-differential.test.ts` 的 `big` 形态核对两侧文案、文件名格式与落盘字节。
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
