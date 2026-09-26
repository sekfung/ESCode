# Rust Bash 模型可见内容

## 背景

Browser Use 第 4 期差分中发现，Rust 前台 Bash 的工具结果以结果对象 JSON 进入模型上下文，形如
`{"stdout":…,"stderr":…,"status":…,"persistedOutputPath":…}`。TS 由
`core/src/tool/handlers/bash-model-content.ts` 的 `formatBashModelContent` 生成纯文本。
Bash 是最常用的工具，这一差异影响每一轮模型请求，也让每次结果都暴露内部落盘路径。

## 规则（对齐 TS）

所有者：Rust 工具层 `tools::tool_process::shell_output` 生成 `ToolOutput.content`；纯格式化规则在
`domain::bash_model_content`。结构化结果（`ToolOutput.data`，行投影用）不变。

1. 提供方错误：`status == "failed"`，退出码为非 0 数字，且返回码解释不是语义非错误
   （`Condition is false`、`Files differ`、`No matches found`、`Some directories were inaccessible`）。
   提供方错误时，正文为以下各行，过滤空行后以换行连接：
   - `Exit code <n>`；
   - stdout 段；
   - stderr 段；
   - 后台说明。
2. 其他情况的正文为以下各段，过滤空段后以换行连接：
   - stdout 段；
   - stderr 段；
   - 后台说明。
3. stdout 段：
   - 去掉开头的空白行，并去掉末尾空白；
   - 结果带落盘路径（非后台）时，改为 `<persisted-output>` 信封：
     - 字节数用 1024 进制，保留一位小数，去掉 `.0`；
     - 预览最多 2000 字符，在后半段的换行处截断，截断时追加 `...`。
4. stderr 段：
   - 取 stderr 并去掉首尾空白；
   - 被终止的命令没有 stderr 文本时，stderr 为 TS 执行错误文案：`Command timed out after <时长>`（TS
     `formatTimeoutDuration`）、`Execution cancelled`、`Execution output exceeded the persisted output limit`；
   - 中断（超时或取消）时追加 `<error>Command was aborted before completion</error>`。
5. 后台说明：
   - 只在有 `backgroundTaskId` 时出现；
   - 文案按 TS 区分三种情况：自动转后台、用户手动转后台、普通后台；
   - 输出路径取单一输出文件。
6. 输出语义（TS `BashFileOutput`）：
   - stdout 与 stderr 写入同一个输出文件；
   - 结果的 `stdout` 为文件前 30,000 字节（TS `MAX_INLINE_OUTPUT_BYTES`），`stderr` 为空；
   - `stdoutBytes` 为文件大小，超过内联上限时 `stdoutTruncated` 为真；
   - Rust 之前分别捕获两路（24 KiB），并额外写 `.stdout`/`.stderr` 文件，改为只写合并文件。
   - Rust 让 stdout 与 stderr 共用一个 OS 管道，由单一读取方写入合并文件，交错顺序与进程实际写入一致。
     分开读两条管道会产生顺序竞争，Linux/macOS CI 上 stderr 曾先于 stdout 落盘。
7. 落盘路径：
   - 前台结果只在输出被截断时保留输出文件，并设置 `persistedOutputPath`、`rawOutputPath`、
     `stdoutPersistedOutputPath` 与对应大小；
   - 未截断时删除输出文件，不设置这些字段，模型正文里也不出现路径；
   - 后台任务保留输出文件路径（TS 同）。
   - 暂不移植：空输出且非 0 退出时的「输出丢失」诊断（`diagnoseLostBashOutput`）。
8. 返回码解释（`interpretBashReturnCode`）：
   - 超时：`Command timed out`；
   - 取消：`Command was cancelled`；
   - 启动失败：`Command failed to start`；
   - 非 0 退出码：`Command exited with code <n>`；
     - 例外：最后一条命令为 grep/rg 等且退出码为 1 时，使用语义解释，例如 `No matches found`；
   - 信号：`Command exited due to signal <sig>`。
9. 暂不移植，保持现状并在差分中排除：
   - `isImage`（stdout 为图片 data URL 时转图片内容）；
   - `ghRateLimitHint`；
   - `staleReadFileStateHint`；
   - 工作目录变化后追加到 stderr 的提示（`appendBashCwdStderrSuffix`）。
10. 结果对象另外带 `returnCodeInterpretation` 与 `noOutputExpected`（TS `isSilentBashCommand`：
    解析成功、无动态词，且每条命令都属于静默命令集，`||` 之后的中性命令除外）。

11. 通用空结果占位（TS `serializeOutput`）：任何工具的模型可见内容为空白且没有媒体时，改为
    `(<工具名> completed with no output)`；被拒绝的调用除外。Rust 在 core 工具分发处统一处理。
12. 后续单独对齐：TS 前台 Bash 超时后不终止，而是转为后台任务（`auto_on_timeout`；以 `sleep` 开头的命令除外），
    模型收到后台说明。见 rust-bash-auto-background.md。

## 验收

- 单测覆盖以下情况：
  - 成功；
  - 空输出；
  - 前导空行；
  - 提供方错误；
  - grep 无匹配（退出码 1，不算错误）；
  - 超时与中断标注；
  - 后台三种文案；
  - 落盘信封的字节格式与预览截断。
- Node 对 Rust 差分：
  - 以下命令的模型可见 tool 消息逐字一致：成功、失败、grep 无匹配、stderr、超时、后台；
  - 大输出落盘的模型正文结构一致（路径不同，按占位比较）。
