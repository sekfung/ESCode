# Rust MCP 对齐：结果格式、图片与 OAuth

2026-09-26。承接 rust-media-read.md 第 5 期与用户选定的「MCP OAuth」缺口。本文先记录差距与分期，实施前逐期补充规则与验收。

## 现状差距（按 TS `core/src/mcp/index.ts` `formatMcpToolResult` 与 `adapters/src/mcp/*` 对照）

| 项                     | TS                                                                                             | Rust 现状                             |
| ---------------------- | ---------------------------------------------------------------------------------------------- | ------------------------------------- |
| 文本块拼接             | 跳过空文本，以 `\n\n` 连接；全是文本时收成字符串                                               | 以 `\n` 连接，不跳过空文本            |
| structuredContent      | 有内容时追加 `Structured content:\n<两空格缩进 JSON>`                                          | 追加紧凑 JSON                         |
| isError                | `MCP tool returned an error:\n<文本>`（server 声明 message-only 时除外）                       | 无前缀                                |
| image                  | 转为 image 块（占位名 `MCP image`），并经 `image-normalization` 做 inline 预算（200KiB）与压缩 | 整块 JSON 文本（base64 进入模型文本） |
| audio                  | `[MCP audio content omitted: <mime>]`                                                          | 整块 JSON 文本                        |
| resource               | `MCP resource content:\n<JSON>`                                                                | 整块 JSON 文本                        |
| 截图 artifact / CUA 帧 | 浏览器截图落 artifact；官方 CUA 帧走 integrity 通道                                            | 无                                    |
| OAuth                  | 交互式授权（浏览器回调）、凭据存储、刷新、租约、官方账号授权（约 2200 行）                     | 无；需要 OAuth 的远程 server 不可用   |

## 分期

1. （已完成）结果格式对齐：文本拼接、structuredContent、isError 前缀、audio/resource 文案；通过 MCP 差分覆盖（`zcode-cli-rust-mcp-result-differential.test.ts`，Chat 与 Anthropic 两种协议）。
   - 缩进 JSON 按 rmcp 类型字段的顺序输出，resource 与 Node 一致。
   - `structuredContent` 在 rmcp 中是无序 Value，多键时键序可能与 server 原序不同（已知差异）。
2. （已完成，部分）图片结果：
   - 走 `ToolOutput.media`，与 rust-media-read.md 第 1 期的通道相同，投影规则见该文；
   - 超过 200KiB inline 预算时暂按 TS「无 artifact store」分支给出省略说明；
   - 落 artifact 与 node_repl 截图压缩尚未实现，归入第 4 期。
3. OAuth：
   - 先写 OAuth 专项 spec，覆盖授权码 + PKCE、回调端口、凭据存储位置与格式（与 Node 共用）、刷新与并发租约、错误分类，以及 Host 侧的交互入口；
   - 与用户对齐后再实施。
4. 截图 artifact 与 CUA 帧：依赖官方插件运行时，单独评估。
