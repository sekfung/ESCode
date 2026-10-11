# Rust runtime 文件日志与保留

2026-10-08。对照 MBearo/ESCode-rs 的 M9.1 发现：Rust 只把少量诊断写 stderr，不写 Node 的 JSONL 日志目录；桌面「导出日志 /
反馈」打包的是 `~/.escode/cli/log`（`packages/desktop/src/main/exportLogs.ts`），Rust 作为默认 runtime 时反馈里没有
runtime 日志，也没有保留清理。

## 规则（对齐 Node `adapters/src/logging`）

- 目录：`ESCODE_LOG_DIR`，缺省 `~/.escode/cli/log`（与 Node 相同，导出日志自动带上）。
- 文件：`escode-rust-YYYY-MM-DD.jsonl`（本地日期）。与 Node 的 `escode-YYYY-MM-DD.jsonl` 分开：两个 runtime 各写各的、
  各清各的，回退或并存时互不删除对方文件。
- 条目：与 Node `toSerializableEntry` 同形，一行一条 JSON，未定义字段省略：
  `timestamp`（ISO UTC）、`level`（debug/info/warn/error）、`event`、`module`、`message`、`sessionId`、`context`。
- 级别：缺省 info；`ESCODE_RUNTIME_ENV=development` 时 debug（Node `getDefaultMinLevel`）。
- 脱敏：`context` 里键名匹配 `api[-_]?key|authorization|cookie|credential|password|secret|token`（不区分大小写）的值替换为
  `[Redacted]`，深度超过 8 替换为 `[Redacted:DepthLimit]`（Node `DefaultLogRedactor`）。
- 写入失败不影响运行（Node：logging must never break the agent execution path）。warn / error 同时写 stderr，保留原有
  诊断输出（Host 仍从 stderr 收集）。
- 保留：7 天。截止日期 = 本地今天 − 7 + 1，早于截止日期的本 runtime 日文件删除；文件名必须是合法日期；目录不存在视为完成。
  启动 60 秒后在后台执行一次（Node `LOG_CLEANUP_STARTUP_DELAY_MS`），调度记 info `log.retention.cleanup.scheduled`，
  完成记 debug `log.retention.cleanup.completed`，失败记 warn `log.retention.cleanup.failed` / `log.retention.delete.failed`。

## 所有者

`escode_cli_host::file_log`：进程级唯一写者（后台线程按序追加），调用方只投递条目、不等待写入。

## 写入点

- 进程生命周期：`runtime.started`（版本、平台）、`runtime.stopped`、致命错误。
- 原有 stderr 诊断（MCP OAuth、官方插件、工作流分析等）改为经 `file_log` 输出。
- 更细的会话 / 工具日志按需逐步补充，不在本项范围。

## 验收

- 单测：条目格式与字段省略、脱敏（含嵌套与深度）、保留的截止日期边界、非法日期与他人文件名不处理、删除失败不影响其他文件。
- App 冒烟：启动 Rust app-server 后日志目录出现当天的 `escode-rust-*.jsonl`，含 `runtime.started`。
