# Rust runtime Windows 对齐（WP2）

2026-09-24。分支此前只在 macOS arm64 验证；在 Windows 11 x64 上跑 App 集成测试暴露以下与 TS 不一致的行为。Shell 选择单独见 [rust-shell-selection.md](rust-shell-selection.md)。

| 问题           | TS 行为                              | Rust 原行为                                                                                       | 处理                                                                                                                   |
| -------------- | ------------------------------------ | ------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| realpath 形态  | `fs.realpath` 返回 `C:\...`          | `canonicalize` 返回 `\\?\C:\...`，进入 prompt、工具输出和路径比较                                 | 统一走 `zcode_cli_host::realpath`（剥离 verbatim 前缀，UNC 转 `\\srv\share`），禁止直接调用 `canonicalize`             |
| 路径拼接       | `path.join` 使用平台分隔符           | `join(".zcode/AGENTS.md")` 生成 `\.zcode/AGENTS.md` 混合分隔符                                    | 所有多段字面量改为逐段 `join`                                                                                          |
| OS Version     | `os.release()` = `10.0.26100`        | `Version.ToString()` = `10.0.26100.0`                                                             | 输出 `major.minor.build`                                                                                               |
| 生成资产换行   | —                                    | `core.autocrlf` 检出 CRLF，漂移检查误报                                                           | `.gitattributes` 固定 LF                                                                                               |
| 记忆模板分隔符 | 运行时追加 `path.sep`                | 资产固化生成机分隔符                                                                              | `{sep}` 占位，运行时填充                                                                                               |
| 只读句柄 fsync | —                                    | 导入附件时 `File::open` 只读后 `sync_all`，Windows 返回 os error 5，整个 TS 导入回滚              | 写入与落盘使用同一可写句柄                                                                                             |
| 产物 URI 解码  | `decodeURIComponent`                 | 借 `file:///` URL 转路径解码，Windows 无盘符必失败（Invalid artifact identity）                   | 直接 percent-decode                                                                                                    |
| 进程树清理     | Windows job object（MCP stdio 已用） | `process_tree` 仅 unix，Windows 只有 `taskkill /T`，Git Bash 的 MSYS 后代失去父链后仍持有输出管道 | Bash 放入 `KILL_ON_JOB_CLOSE` Job（`crates/tools/src/win_job.rs`），终止时 `TerminateJobObject`，附加失败退回 taskkill |

## 待确认：代理环境变量

reqwest 默认读取 `HTTP(S)_PROXY`，开发机设置代理且无 `NO_PROXY` 时，发往本地 `127.0.0.1` 模型 fixture 的请求被转给代理并挂起。测试 fixture 已显式设置 `NO_PROXY=127.0.0.1,localhost` 保持隔离。生产语义需在 WP4 与 TS `adapters/src/network/proxy-fetch.ts`（`network.httpProxy` / `ZCODE_HTTP_PROXY` / `noProxy` / CA）逐项比对后决定，不在此处用兜底掩盖。

## 验收

- `pnpm check:zcode-cli-rust` 与 `pnpm test:zcode-cli-rust` 在 Windows 通过（结果见提交说明）。
- 仍以 `skip: win32` 跳过的集成用例逐个评估，能在 Git Bash 下运行的解除跳过。

## 验收记录（2026-09-24，Windows 11 x64，Node 26.7 非锁定版本）

- `pnpm check:zcode-cli-rust`：通过（boundaries、fmt、clippy `-D warnings`）。
- `pnpm test:zcode-cli-rust`：192 个 App 集成用例，173 通过、18 跳过（多为 `skip: win32`，待逐个评估）、1 失败。
- 未解决：`zcode-cli-rust-registry.test.ts` 的 provider 配置热更新用例只在全量并发运行时偶发超时（单独运行 5/5、整文件 3/3 通过），疑似负载下的配置刷新时序问题，需定位根因，不以加长超时处理。
