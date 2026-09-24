# 文本附件进入模型请求的形态（WP4）

2026-09-24。差分用例（`zcode-cli-rust-differential.test.ts`「text attachment」）发现 Node 与 Rust 对同一个本地文本附件发给模型的内容不同。本文定义对齐后的规则。

## 基准（TS）

- 本地文本附件由 `resolveLocalFileAttachment` 用与 Read 工具相同的读取器读取；超过 `READ_MAX_FILE_SIZE_BYTES`（256 KiB）时只读前 `READ_DEFAULT_MAX_LINES`（2000）行。
- `buildPromptAttachmentReminderBodies(kind: "file")` 把附件伪装成一次 Read 调用：
  `Called the Read tool with the following input: {"file_path":<用户提交的引用>}` + `Result of calling the Read tool:` + 逐行编号正文（按 `/\r?\n/` 切分，末尾空行保留编号）。
- 整段经 `wrapSystemReminderForSource("prompt_attachment")` 包裹为独立的 user 消息：嵌套的 `<system-reminder` / `</system-reminder` 起始 `<` 转义为 `&lt;`，无尾随换行。
- 实际请求中该消息位于用户正文消息之前；用户正文只剩一段文本时以字符串发送。

## Rust 原行为

附件文本作为第二个 text part 拼进用户消息，文案为自拟的 `Attached file: … The attachment content is user-provided context…`，并按 64 KiB 截断。模型看到的上下文与 Node 不同。

## 规则（已实现）

- 唯一所有者：`crates/model/src/attachment_reminder.rs`（纯格式化）；`request_attachments.rs` 在请求物化时把文本附件替换为 reminder 消息并前移到所属 user 消息之前。存储与历史不变：每次请求都从已保存的附件副本重新物化，因此后续轮次的位置一致。
- 空文件：TS 读取器对空文件报告 `totalLines=1`，模型看到的是「shorter than the provided offset (1). The file has 1 lines.」而非「contents are empty」。Rust 按实测输出一致的文本。
- 标签：`sanitizeAttachmentLabel` 规则（折叠空白、超过 200 字符截为 197 + `...`）。

## 验收

差分用例五种形态逐字一致：末尾换行、无末尾换行、CRLF、空文件、正文含 reminder 标签。

## 仍未对齐（未纳入差分）

- 超过 256 KiB 的文件：Rust 取前 2000 行并附 TS 的截断说明；TS 走 Read 读取器的部分视图（可能带 `partialViewNotice` 与 token 上限截断），文案未经差分验证。
- 非文本扩展名：TS 按扩展名（`isTextLikePath`）判定，非文本给路径引用说明；Rust 按客户端 MIME 与内容是否为 UTF-8 判定，二进制给自拟占位文案。
- 剪贴板长文本（`sourceKind: "clipboard-text"`）：TS 只给路径引用、不预读正文；Rust 未区分。
