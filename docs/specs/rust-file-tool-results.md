# Rust 文件工具结果与工具错误的模型可见文案

## 背景

2026-09-27 内置工具差分探测（Read/Write/Edit 共 15 个场景）显示，除入参校验外，文件工具的结果与错误文案
和 Node 不同：

- 成功文案用解析后的绝对路径，缺少新建与全部替换两种变体，也缺少「无需回读」后缀；
- 所有工具错误都带 `Tool failed: ` 前缀，还夹带内部错误码（如 `edit_old_string_not_found: …`）；
- 文件不存在时给出操作系统错误，没有「Did you mean」建议；
- 目录与空文件的文案不同。

## 规则（对齐 TS）

1. 工具错误的模型可见形式（TS `createErrorResult` / `stringifyToolResultOutput`）：
   - 工具处理器失败（TS `ToolHandlerFailure`）：`<tool_use_error>{message}</tool_use_error>`；
   - 其余抛出的错误：错误消息，不加前缀，经 TS `sanitizeText` 处理：
     - 折叠连续空白、去掉首尾空白；
     - 超过 500 个 UTF-16 码元时截为 497 加 `...`。
   - 同批对齐的其他工具文案：
     - TaskOutput 找不到任务：`No task found with ID: {id}`，处理器失败；
     - TaskStop 找不到任务：同一文案，普通错误；
     - Skill 找不到：`Skill not found: {name}`；
     - WebFetch 的 URL 无法解析：zod `url()` 的 issue JSON，先于 URL 规范化检查。
   - Todo 结果的 JSON 键顺序同 TS 对象字面量：`oldTodos, todos, summary`，条目为 `content, status, priority`。
   - Rust：工具以 `core_api::ToolHandlerFailure(message)` 表达处理器失败，`tool_dispatch` 按上面两种形式输出。
2. 路径：成功文案与 `filePath` 使用模型给出的原始 `file_path`（TS `filePath: file_path`）。读写状态与检查点仍用归一化路径。
3. Write：
   - 新建：`File created successfully at: {path}` 加后缀；
   - 覆盖：`The file {path} has been updated successfully.` 加后缀；
   - 后缀为 ` (file state is current in your context — no need to Read it back)`；
   - 未读先写：`File has not been read yet. Read it first before writing to it.`，普通错误；
   - 读后被改：`File has been modified since read, either by the user or by a linter. Read it again before attempting to write it.`，普通错误。
4. Edit：
   - `old_string == new_string` 最先检查：`No changes to make: old_string and new_string are exactly the same.`；
   - 文件不存在且 `old_string` 非空：缺失文件文案（见 6）；
   - 未读与读后被改：文案同 Write，均为处理器失败；
   - 找不到与多处匹配：去掉内部错误码后的原文，均为处理器失败；
   - 成功：`The file {path} has been updated successfully.`；`replace_all` 时为
     `The file {path} has been updated. All occurrences were successfully replaced.`；均加后缀。
     `old_string` 为空时新建文件，同样用普通成功文案。
5. Read：
   - 文件不存在：缺失文件文案，普通错误；
   - 目录：`Cannot read directory as text file: {解析后的绝对路径}`，普通错误；
   - 空文件与 TS adapter 一样计为 1 行，因此给出「shorter than the provided offset (1). The file has 1 lines.」提醒。
6. 缺失文件文案（TS `createMissing{Read,Edit}FileMessage`）：
   - 正文：`File does not exist. Note: your current working directory is {cwd}.`；
   - 同目录存在相近文件名时追加 ` Did you mean {name}?`；
   - 相近文件名的规则：
     - 候选为同目录的文件与符号链接，不含目标本身，按名称排序；
     - 先取主名相同（去扩展名，按 Node `extname` 规则）的第一个；
     - 否则取 Levenshtein 距离 ≤ 3 的第一个（按 UTF-16 码元计算）。

7. 重复 Read（TS `isCachedReadFresh`）：
   - 按（路径、offset ?? 1、limit）记录上次非 partial view 读取时的 mtime（整数毫秒）与大小；
   - 再次读同一范围且二者都未变时，返回
     `Wasted call — file unchanged since your last Read. Refer to that earlier tool_result instead.`；
   - Write/Edit 之后按整文件记录（TS `updateReadFileStateAfterWrite`），Bash 回填按其范围记录；
   - 图片、视频、PDF 在去重之前按媒体分支处理。
8. Write 覆盖已存在但为空的文件时按新建处理（TS `if (originalFile)`），文案与 `type` 都为 create。

## 不在本次范围

- `userModified`（用户在确认时改写内容）：Rust 没有这一交互，文案按未改写处理。
- 大结果写 artifact 的附注（Rust 特有），保持现状。

## 验收

- 单测：相近文件名规则；成功文案的四种变体。
- App 差分：探测脚本中的 15 个 Read/Write/Edit 场景在两侧逐字一致（临时目录路径按占位比较）。
