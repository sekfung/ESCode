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

## 大文件与非文本（2026-09-24 已对齐）

- 读取语义对齐 TS `readTextFileForModel` + adapters `readTextFileRange` 快路径（`crates/model/src/attachment_read.rs`）：CRLF 归一后按换行符切行；文件超过 256 KiB 只取前 2000 行；估算 token（UTF-16 长度，`[一-鿿]` 计 2，除以 3 向上取整）超过 25000 时，二分取不超过 21250 的最长行前缀，并以 `The file is too large to display in full (…)` 部分视图提示开头。
- 按扩展名（TS `isTextLikePath`）判定是否读入正文；非文本扩展名交付路径引用说明（`Attached <按扩展名推断的 mime>: <引用>` + 固定两行），留在用户消息内、位于正文之后。媒体（image/pdf/video）仍先按 MIME 走媒体分支。
- 差分用例：二进制文件、未知扩展名的文本、超过 256 KiB 的文本，与 Node 逐字一致（完整原文 sha256）。

## 仍未对齐

- 文本扩展名但内容无法按 UTF-8 解码：TS 按编码探测解码（可能读出 latin1/UTF-16 文本）；Rust 给自拟的二进制占位文案。
- V4 附件引用（`attachmentRefSchema`）不携带来源类型，TS 的剪贴板长文本（`sourceKind: "clipboard-text"`）延迟读取分支在 App 协议下不可达，无需对齐。
