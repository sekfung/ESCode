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

工具链：本机无 MSVC，改用 `stable-x86_64-pc-windows-gnu` + MSYS2 GCC 构建（见下节）；
产物是 **GNU 目标**，结论为源码级证据，发布验收仍需 MSVC 重跑。

- Rust 测试：`cargo +…-gnu test --workspace -- --test-threads=1` 全部通过（0 失败）。
  并发跑同一套会出现超时假失败（每文件起一个 runtime + HTTP fixture，本机 3s 的 receive 超时会被压满）。
- App 集成：`node --import tsx --test --test-concurrency=1 packages/services/tests/zcode-cli-rust-*.test.ts`
  （须带 runner 的 `TSX_TSCONFIG_PATH`，否则 `@zcode/*` 解析失败）→ **196 用例：194 通过、2 跳过、0 失败**。
- 跳过用例：**2 条**（原 13 条）。改写方式：用「心跳文件」替代 POSIX PID 断言——shell 循环追加时间戳，
  进程（含子进程）被回收后文件不再增长；`GIT_TRACE2_EVENT` 替代无扩展名的伪造 git 脚本；
  Windows 上没有 SIGTERM 的用例改用 stdin EOF 或 Job Object 语义。仍跳过的两条：
  1. prompt 的慢 git 探测：需要伪造 PATH 上的 `git`，而 Windows 的 CreateProcess 只解析 `.exe`，属测试夹具限制；
  2. transport 的输出背压：EOF 在背压下不可观测，属已确认缺陷（见 rust-runtime-performance.md）。
- 本轮修复：`os_release` 不再 spawn PowerShell（首个请求 1830ms → 35ms，见下节）；
  导入源库的 busy timeout 由 20ms 放宽到 5s（两个 runtime 同时启动时的瞬时锁竞争）；
  权限确认相关的既有用例按 build/edit 新契约改写（stdio / migration / host）。

## 无 MSVC 时的可运行验证路径

本机 2026-09-24 缺 MSVC/SDK（`VC\Tools\MSVC`、`Windows Kits\10\Lib` 均不在），`link.exe` 不可用；
而 rustup 装了 `stable-x86_64-pc-windows-gnu`，MSYS2 提供 mingw64 的 gcc/ld，因此可以：

```
PATH=/c/msys64/mingw64/bin:$PATH cargo +stable-x86_64-pc-windows-gnu build --locked --offline --target x86_64-pc-windows-gnu
cp target/x86_64-pc-windows-gnu/debug/zcode-cli-rust.exe target/debug/zcode-cli-rust.exe
```

`packages/services/tests/zcode-cli-rust-fixture.ts` 固定读取 `target/debug/zcode-cli-rust.exe`，替换后即可跑 App 集成套件。

注意：这是 GNU 目标产物，与发行用的 MSVC 目标不同（CRT 与部分平台行为有差异）。用它得出的是**源码级**结论，
不能替代 MSVC 构建的发布验收；MSVC 环境恢复后必须重跑并以此为准。

## 性能：prompt 快照不再启动 PowerShell

`os_release` 原先在 Windows 上 spawn `powershell.exe` 取版本号。本机一次 PowerShell 启动约 1.5s，
而这段位于 prompt 快照的关键路径，导致**首个模型请求延迟约 1.83s**（实测三次 1830/1827/1830 ms），
并使「1s 内发出首个请求」的用例必然超时。

改为读注册表 `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion` 的
`CurrentMajorVersionNumber` / `CurrentMinorVersionNumber` / `CurrentBuildNumber`，
输出仍是 Node `os.release()` 的 `major.minor.build`（如 `10.0.26100`）。

实测：首个请求 1830ms → **35ms**（36/34 ms）；provider 的 stop 用例由必然失败变为 515ms 通过；
prompt 用例（含 `OS Version: win32 10.0.26100 x64`）保持通过。

## Linux 验证（2026-09-24）

把 Linux 目标依赖（`linux-raw-sys` 等）先在 Windows 上 `cargo fetch --target x86_64-unknown-linux-gnu` 取进共享的
cargo 缓存，WSL Ubuntu 即可离线构建并运行整套 Rust 测试：

```
# Windows 侧取依赖（一次性）
cargo fetch --target x86_64-unknown-linux-gnu
# WSL 侧运行
CARGO_HOME=/mnt/c/Users/sekfung/.cargo CARGO_TARGET_DIR=$HOME/zcode-target \
  cargo test --offline --workspace -- --test-threads=1
```

结果：**29 个测试二进制的 Rust 测试全部通过、0 失败**（含 `crates/tools` 的 git 安全语料与 POSIX 分支、
`crates/domain` 的权限/规则/代理解析差分，以及 app-server/core/state/model/host）。
这覆盖了 POSIX 专属路径（进程组回收、`/bin/bash` 选择、`uname` 版本探测）。

跨平台现状：Windows（MSVC 目标未跑，用 GNU 目标）与 Linux 均已通过 Rust 测试与（Windows 侧）App 集成套件；
**macOS 未验证**（本机无 macOS 环境），发布验收仍须在 MSVC 目标与三平台原生环境重跑。
