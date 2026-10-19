# Rust 迁移 M2 报告：Host 韧性与故障隔离（2026-10-18）

## 1. 基线与范围

| 项         | 值                                                                                                                                           |
| ---------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| 分支       | `m2/host-resilience`（从 `m1/gates-and-r-line` `42c80e06` 新建）                                                                             |
| 开工 HEAD  | `42c80e06fabbd857eab465af6b71c2edc3ac211e`（2026-10-18 04:23:40 +0800）                                                                      |
| 收尾 HEAD  | `fa109b70`（代码最后一个提交，2026-10-18 05:36:39 +0800）+ 本报告提交                                                                        |
| 平台       | Linux x86_64（VMware，7.0.0-34-generic），无图形会话                                                                                         |
| Rust       | rustc 1.95.0（59807616e 2026-04-14），cargo 1.95.0（与 `rust-toolchain.toml` 一致）                                                          |
| Node/pnpm  | v24.14.0 / 10.33.2（`mise.toml` 期望 Node 24.18.0，本机有 engine 警告）                                                                      |
| freshness  | `node scripts/check-workspace-freshness.mjs` exit 0：相对 origin/main ahead 178 / behind 0；本分支无远端跟踪分支                             |
| 开工工作区 | `git status --short` 有 5 个他人未提交改动（zip 8.6→9.0.5 依赖升级与 `ooxml.rs`）；本轮未提交、未修改它们                                    |
| 对标仓库   | ZCode-rs `git clone --depth 1` 到会话 scratchpad（工作区外），commit `909c6554`（2026-10-11），只读阅读 `docs/specs/rust-host-resilience.md` |

### 并发工作流说明

本轮进行中，另一条工作流在**同一工作区、同一分支**上提交了三个与 M2 无关的提交，本报告未修改、未回退它们：

- `6e130688 fix(deps): 升级 rustls 与 zip 清除 cargo deny 安全公告`：即开工时那 5 个未提交改动（zip 9.0.5、`ooxml.rs`、两份 `Cargo.lock`），外加 rustls 升级。
- `f6996652 docs(reports): M1 复核收口，R 线 ACCEPT`：改了 M1 报告与验收报告顶部的“后续”行。
- `0684cdff ci(rust): checks 扩到三平台，Host 工作区启用 cargo deny`：只改 CI 与 deny 配置，位于 `6ff53d7e` 与 `64d1bd12` 之间。

因此 T2 之前的本地构建使用的是“HEAD + 他人未提交的 zip 9.0.5 改动”；`6e130688` 之后与提交内容一致。M2 的 T 提交只包含本轮文件（`apps/escode-cli-rust/Cargo.toml` 只暂存了 `[features]` 一段，zip 那一行留在工作区，见 §10）。另有其它工作流的工作树 `/tmp/escode-head-check`、`…/7deb3a96-…/scratchpad/m1-wt`，本轮未触碰。

### 跑过的命令与结果（收尾时）

| 命令                                                                                                                                                                            | 退出码 | 关键结果                                                                                                                                                 |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------ | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `pnpm check:escode-cli-rust`                                                                                                                                                    | 0      | 边界 OK、fmt、clippy `-D warnings` 通过                                                                                                                  |
| `pnpm check:escode-host`                                                                                                                                                        | 0      | `Rust Host boundaries: OK`；`service inventory: OK (46 channels, 539 members)`；fmt、clippy 通过                                                         |
| `pnpm test:escode-host`                                                                                                                                                         | 0      | 56 个测试二进制，728 passed / 0 failed；interop fail 0                                                                                                   |
| `ESCODE_TEST_SERIAL=1 ESCODE_TEST_REQUEST_MS=60000 ESCODE_TEST_EXIT_MS=15000 pnpm test:escode-cli-rust`                                                                         | 0      | Rust（`cargo test`，本机无 nextest）61 个测试二进制 464 passed / 0 failed；TS 411 项 399 pass / 0 fail / 12 skipped；耗时约 31 分钟（05:40:13–06:14:05） |
| `pnpm typecheck`                                                                                                                                                                | 0      | —                                                                                                                                                        |
| `pnpm lint`                                                                                                                                                                     | 0      | 0 error / 160 warning（与 M1 相同）                                                                                                                      |
| `pnpm fmt:check`                                                                                                                                                                | 0      | 8693 文件格式正确                                                                                                                                        |
| `pnpm architecture:check --changed`                                                                                                                                             | 0      | violations 0 / baseline 0 / new 0                                                                                                                        |
| `node scripts/generate-escode-host-service-inventory.mjs --check`（含于 check:escode-host）                                                                                     | 0      | 46 channels / 539 members                                                                                                                                |
| `pnpm exec vitest run packages/desktop/test/{rustHostResilience,rustHostBridge,rustHostRpc,webRemoteControlSharedHostAttachments,webRemoteControlManagerExternalRelay}.test.ts` | 0      | 5 文件 87 passed                                                                                                                                         |

## 2. T0：M1 遗留 5 个失败的定性（bisect）

做法：在独立 worktree（`git worktree add --detach <scratchpad>/wt-m1base 33cf6b34`，M1 之前的 `dev`）里：

| 步骤                    | 命令                                                                                                                                                                                                                                                                                                                                                                                                         | 退出码 |
| ----------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------ |
| 依赖                    | `CI=1 pnpm install --offline --frozen-lockfile --ignore-scripts`                                                                                                                                                                                                                                                                                                                                             | 0      |
| Rust CLI（独立 target） | `CARGO_TARGET_DIR=<wt>/apps/escode-cli-rust/target cargo build --locked --manifest-path apps/escode-cli-rust/Cargo.toml`（2m21s，产物 04:47）                                                                                                                                                                                                                                                                | 0      |
| Node CLI                | `generate-libs.mjs` → `pnpm --dir apps/escode-cli --filter '@escode/bootstrap^...' build` → `tsc -b packages/rpc … packages/services` → `node scripts/build-desktop-agent-cli.mjs`（worktree 自己的 `apps/escode-cli/packages/cli/dist/escode.cjs`，15080059 字节，04:50）                                                                                                                                   | 0      |
| 4 个测试文件            | `ESCODE_TEST_SERIAL=1 ESCODE_TEST_REQUEST_MS=60000 ESCODE_TEST_EXIT_MS=15000 TSX_TSCONFIG_PATH=<wt>/packages/services/tests/tsconfig.escode-cli-rust.json node --import tsx --test --test-concurrency=1 --test-timeout=300000 --test-force-exit packages/services/tests/escode-cli-rust-{mcp-request-context,model-io,plugins-install,plugins-list}.test.ts`（与 `scripts/test-escode-cli-rust.mjs` 同口径） | **1**  |

结果：`tests 5 / pass 0 / fail 5`，失败摘要与 M1 报告 §4 逐条相同：

| 用例                                                           | `33cf6b34`（M1 前） | 失败摘要                                                                                               | 结论 | 修复                                                                                                                     |
| -------------------------------------------------------------- | ------------------- | ------------------------------------------------------------------------------------------------------ | ---- | ------------------------------------------------------------------------------------------------------------------------ |
| `mcp-request-context`：MCP tools/call 带与 Node 相同的 `_meta` | FAIL                | `ENOENT …/mcp-normal-calls`：模型桩选中了随包官方插件的 `mcp__` 工具，fixture 的 `local.echo` 未被调用 | 既存 | 已由 `42c80e06`（M1 分支，M2 开工前）修复：测试按 fixture server 名精确选取                                              |
| `model-io`：App 轨迹面读取 model-io                            | FAIL                | `toolNames` 中 `mcp__node_repl__js` 排在 `mcp__plugin_usb_*` 之前（Node 在之后）                       | 既存 | `42c80e06`：MCP server 按 JS 对象 spread 键序合并（`mcp_config_order.rs`）                                               |
| `model-io`：full model-IO retention                            | FAIL                | 同上                                                                                                   | 既存 | 同上                                                                                                                     |
| `plugins-install`：本地源安装                                  | FAIL                | 依赖缺失回 `plugin_marketplace_invalid`，Node 为 `plugin_dependency_missing`                           | 既存 | `42c80e06`：`Dependency not found` 改抛 `Kind::DependencyMissing`；引入点 `87f7aa44`（2026-10-15，是 `33cf6b34` 的祖先） |
| `plugins-list`：插件发现字段                                   | FAIL                | 同一插件 skill 集合相同、顺序不同（Rust `read_dir` 原始顺序 vs Node 字节序）                           | 既存 | `42c80e06`：技能扫描复用 `plugin_describe_scan::entries` 的字节序                                                        |

结论：5 个失败在 M1 之前的提交上同样失败，**不是 M1 回归**；M1 报告“既存”的判定成立（但 M1 报告当时给出的依据是“不在改动路径”，本轮补上了 bisect 证据）。修复已在 M2 开工前由 `42c80e06` 完成，M2 不重复修改；在 M2 收尾 HEAD（`fa109b70`）上完整套件 exit 0，这 5 例全部通过（§1、§3）。

worktree 清理：`git worktree remove --force <scratchpad>/wt-m1base && git worktree prune`；之后主工作区 `git status --short` 只剩开工时那 5 个他人改动（与开工一致），`git worktree list` 不再含 `wt-m1base`。

## 3. 逐项结论

| ID                                            | 结论 | 证据（命令+退出码 或 文件:行号）                                                                                                                                                                         | 备注                                                                                                                        |
| --------------------------------------------- | ---- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------- |
| T0                                            | PASS | §2：`33cf6b34` 上 4 文件 5 例 exit 1、fail 5                                                                                                                                                             | 5 个均为既存；修复在 `42c80e06`                                                                                             |
| T1 spec                                       | PASS | `docs/specs/rust-u2-host-resilience.md`（提交 `4f0de02c`）                                                                                                                                               | §1 逐条写明对 D2 的采纳/不采纳；§7 所有者与事件顺序图                                                                       |
| H2.7 embedded                                 | PASS | `cargo test -p escode-host-runtime --test crash_isolation` exit 0（`embedded_panic_in_one_workspace_leaves_the_other_untouched`）；连跑 10 次 10/10                                                      | A 注入 `test/panic`；断言见 `crash_isolation.rs:84` 起的 `crash_isolation`（两条链路共用）                                  |
| H2.7 child                                    | PASS | 同上（`killed_child_in_one_workspace_leaves_the_other_untouched`），`kill -9 <pid>`；`--nocapture` 下 skip 计数 **0**                                                                                    | Windows 分支用 `taskkill /F`（TerminateProcess），本机未执行（UNVERIFIED：无 Windows 环境）                                 |
| H2.7 负向                                     | PASS | §4：三种变异全部 exit 101                                                                                                                                                                                | —                                                                                                                           |
| H2.2 前半                                     | PASS | `rustHostResilience.test.ts` 启动失败 6 例 + manager 日志 1 例，exit 0；连跑 5 次 5/5                                                                                                                    | 四类各有分类码；截止时间本身是被断言的分类（`RUST_HOST_HANDSHAKE_TIMEOUT`）                                                 |
| Host 进程崩溃                                 | PASS | `rustHostResilience.test.ts` “rust host process crash” 4 例 exit 0（上限内恢复 + attachment 重建、超限放弃、窗口外不计数、stop 不算意外）                                                                | 真实 `escode-host` 进程 SIGKILL；退避用注入时钟断言 `[500, 2000, 5000]`                                                     |
| H1.3                                          | PASS | `cargo test -p escode-host-rpc --test rpc_order` exit 0（修复后连跑 5 次 5/5）；修复前同一测试 3/3 失败（“登记序必须等于到达序”）                                                                        | 发现并修复真实缺陷：处理函数过去在独立任务里才被调用（`connection.rs` / `session.rs::start`）                               |
| H1.4                                          | PASS | `docs/specs/rust-u2-host-skeleton.md` §3.2 表逐队列列常量与构造点；`host-control/src/lib_tests.rs` `full_queue_drops_logs_of_every_level_and_counts_them`；`cargo test -p escode-host-control` 16 passed | 取 (b)：队满任何级别都可能丢并计数；删除“Gateway 入站 256 背压”                                                             |
| `check:escode-cli-rust` / `check:escode-host` | PASS | 均 exit 0                                                                                                                                                                                                | 首跑 `check:escode-host` 被 400 行规则拦下（M2 新增的 3 个文件），已拆分（`fa109b70`）                                      |
| `test:escode-host`                            | PASS | exit 0，728 passed / 0 failed                                                                                                                                                                            | 首跑发现 M2 引入的 `rpc_agent_v4` 栈溢出，已修（§6）                                                                        |
| `test:escode-cli-rust`                        | PASS | exit 0；Rust 464/0；TS 411 项 399 pass / 0 fail / 12 skip                                                                                                                                                | T0 的 5 例在 HEAD 全部通过（✔ mcp-request-context、model-io ×2、plugins-install、plugins-list）；无剩余失败                 |
| typecheck / lint / fmt                        | PASS | 三者 exit 0                                                                                                                                                                                              | `pnpm typecheck` 不覆盖 `packages/desktop/tsconfig.main.json`；对该配置单独跑 tsc，M2 前后都是 91 个既存错误，无新增（§10） |
| architecture                                  | PASS | `pnpm architecture:check --changed` violations 0                                                                                                                                                         | —                                                                                                                           |
| inventory `--check`                           | PASS | exit 0（46 channels / 539 members）                                                                                                                                                                      | —                                                                                                                           |

### 3.1 H2.7 的断言清单（两条链路同一函数 `crash_isolation`）

1. A、B 各有一个进行中的 `workspace/generateText`：模型桩收到 HTTP 请求才继续，因此请求确定地停在 runtime 内（通道事件，不用 sleep）。
2. 让 A 崩溃（embedded：`test/panic`；child：SIGKILL）。
3. A 的在飞请求与 `test/panic` 本身都以类型化 `RuntimeClosed` 失败，文案逐字为 `ESCode agent transport closed: stdout_closed`（TS `escodeProtocolClient.ts:126` + `escodeStdioTransport.ts:234`），不等 60 s 请求超时。
4. A 的订阅流结束，`lagged() == false`、`is_closed() == true`；生命周期流收到且只收到 `Unavailable(A, gen 1)`。
5. 崩溃回收不等下一次 `get`：`instance.exited()` 给出 `requested: false`、`residual: []`；embedded 原因逐字为 `engine panicked: fault-injection: test/panic`，child 为 `signal: 9 (SIGKILL)`。
6. `instances.live(A)` 为 `None`。
7. B：放行模型桩后在飞请求返回 `text == "from B"`，后续 `session/list` 成功，订阅未关闭，代次仍为 1。
8. A 下一次 `get` 懒重启为第 2 代，`session/list` 成功，生命周期收到 `Available(A, gen 2)`；之后 `try_recv` 为空（B 没有任何事件）；`generations() == {A: 2, B: 1}`。
9. `instances.shutdown()` 残余为空。

故障注入点：`apps/escode-cli-rust/crates/core/src/app/engine_requests.rs:13-19`，`#[cfg(feature = "fault-injection")]`；feature 定义在 `crates/core/Cargo.toml`，只由 `apps/escode-host/crates/host-runtime/Cargo.toml` 的 `[dev-dependencies]` 打开，`cargo build` / release 不启用。

## 4. 负向验证记录

全部在独立 worktree（`git worktree add --detach <scratchpad>/wt-m2-neg 818c040f`，`CARGO_TARGET_DIR=<scratchpad>/neg-target`；worktree 里的 `apps/escode-cli-rust/target/debug/escode-cli-rust` 软链到主工作区本轮构建的 CLI，child 用例因此也执行）：

| #   | 变异                                                                            | 命令                                                                 | 结果                                                                                                         |
| --- | ------------------------------------------------------------------------------- | -------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------ |
| 1a  | 删掉 `host-runtime/src/lib.rs` 的 `slot.generation += 1`                        | `cargo test -p escode-host-runtime --test crash_isolation`           | EXIT 101；两例都在首代断言失败（left 0 / right 1）——变异过宽，连首代都不对，改用 1b                          |
| 1b  | 懒重启不递增：只有首代取 1（`if slot.generation == 0 { slot.generation = 1 }`） | 同上                                                                 | **EXIT 101**；两例在 `crash_isolation.rs:352` `assert_eq!(restarted.generation, 2)` 失败（left 1 / right 2） |
| 2   | 不清理崩溃实例：`Instances::live` 不再检查 `is_alive()`                         | 同上                                                                 | **EXIT 101**；两例在 `assert!(instances.live(&a.identity()).is_none())` 失败                                 |
| 3   | 不回收崩溃实例：帧泵读端关闭后不调 `reap()`                                     | 同上                                                                 | **EXIT 101**；两例在 `A reaped within guard: Elapsed(())` 失败（60 s 失败保护到点）                          |
| 4   | H1.3：修复前代码（`connection.rs` 在独立任务里调用处理函数）                    | `cargo test -p escode-host-rpc --test rpc_order`（主工作区，修复前） | **3/3 EXIT 101**：`登记序必须等于到达序`、`连接 0 内部登记序 = 到达序`                                       |

（行号为负向验证时 `818c040f` 的 `crash_isolation.rs`；`fa109b70` 拆分后两条断言分别在 `crash_isolation.rs:200` 与 `:173`。）

清理证明：

```text
$ git worktree remove --force <scratchpad>/wt-m2-neg && git worktree prune && git worktree list
/home/sekfung/Projects/ESCode                                       7220678d [m2/host-resilience]
…/7deb3a96-…/scratchpad/m1-wt                                       8f38ecfd [m1/gates-and-r-line]   ← 其它工作流
/tmp/escode-head-check                                              1f885ca4 (detached HEAD)          ← 其它工作流
$ rm -rf <scratchpad>/neg-target
$ git status --short | wc -l
0
```

## 5. 实现摘要（提交）

| 提交       | T     | 内容                                                                                                                                                                                                                     |
| ---------- | ----- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `4f0de02c` | T1    | 新增 `rust-u2-host-resilience.md`                                                                                                                                                                                        |
| `818c040f` | T2    | 帧泵唯一检测者 + 即时崩溃回收（`ExitReport`、error 日志）；`RuntimeClosed`（Node 文案，host-rpc 映射 `name=Error`）；`Lifecycle::wait` 幂等；embed 根任务 `catch_panic`；`fault-injection` feature；`crash_isolation.rs` |
| `8c8b67d6` | T3    | `RustHostStartError` 四类、单一截止时间、监听子进程 `error`（修复二进制缺失时 Main 未处理异常）；manager 记 error 并原样抛出；`RUST_HOST_*` → `desktop-disconnected`                                                     |
| `57ea5b0d` | T4    | manager 状态机、60 s/3 次/500 ms·2 s·5 s 受限重启、`RUST_HOST_CRASH_LOOP`、`resetRestartBudget`；attachment `onLost` → 远控 `handleHostAttachmentLost` → `bridge-degraded`（replayable 恢复重建）                        |
| `6ff53d7e` | T2 补 | 修复 `818c040f` 引入的 `rpc_agent_v4` worker 栈溢出（§6）                                                                                                                                                                |
| `64d1bd12` | T5    | `session::start` 读循环内同步登记；`rpc_order.rs`；骨架 spec §3.4/§8                                                                                                                                                     |
| `7220678d` | T6    | 骨架 spec §3.2 重写；`ControlWriter::dropped_logs()` 与全级别丢弃计数测试                                                                                                                                                |
| `fa109b70` | 门禁  | 拆分 `instance.rs` / `crash_isolation.rs` / `host-control/lib.rs` 守住 400 行                                                                                                                                            |
| （本报告） | 文档  | M2 报告、骨架 spec §6 回退表述订正、验收报告“后续”链接                                                                                                                                                                   |

## 6. 过程中发现并修复的回归

`pnpm test:escode-host` 首跑时 `host-rpc/tests/rpc_agent_v4.rs` 以 `thread 'tokio-rt-worker' has overflowed its stack` 中止（SIGABRT）。bisect：`42c80e06` 通过、`818c040f`（T2）失败。原因：T2 给 embed 实例根任务套上 `catch_panic` 后，debug 构建下巨大的实例 future 在 2 MiB 的 tokio worker 栈上多了一份临时副本。修复（`6ff53d7e`）：`Box::pin(run_embedded(...))` 后再包裹；修复后 `rpc_agent_v4` 连跑 3 次通过，`crash_isolation` 仍通过，完整 `test:escode-host` 728/0。

## 7. M5 回退分支删除清单（本轮不删）

以收尾 HEAD 的行号为准：

| 位置                                                                         | 内容                                                                                                                            |
| ---------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| `packages/desktop/src/main/rustHostBridge.ts:81-85`                          | `isRustHostEnabled()`：读 `ESCODE_RUST_HOST`                                                                                    |
| `packages/desktop/src/main/rustHostBridge.ts:4-8`                            | 文件头“dev 开关”注释                                                                                                            |
| `packages/desktop/src/main/rustHostManager.ts:134`                           | `enabled = options.enabled ?? isRustHostEnabled()`                                                                              |
| `packages/desktop/src/main/rustHostManager.ts:40`、接口 `enabled` 字段       | 开关覆盖选项与“关闭时调用方走 Node Host”                                                                                        |
| `packages/desktop/src/main/index.ts:866-886`                                 | `...(rustHostManager.enabled ? { attachRustHostLocal, onHostAttachmentLost } : {})`：开关关闭即不注入 Rust attachment           |
| `packages/desktop/src/main/index.ts:836-840`                                 | 对应注释（已改为“Node Host 路径属 M5 删除清单”）                                                                                |
| `packages/desktop/src/main/webRemoteControlSharedHostAttachments.ts:144-168` | `attachRustHostLocal ? … : undefined` → `return attachLocalHost(windowId)`（`:168`）：无 Rust attachment 时走 Node utility Host |
| `packages/desktop/src/main/webRemoteControlSharedHostAttachments.ts:107`     | `attachLocalHost`（Node utility Host 的 `AttachServicePort`）                                                                   |
| `apps/escode-host/src/main.rs:22`、`:30`、`:350`                             | 注释“不匹配时 Main 应回退 Node Host”“Main 据此决定重试或回退”“（阶段 3）回退”                                                   |
| `packages/desktop/test/rustHostBridge.test.ts:33`、`:153`                    | 开关判真 / 关闭的用例                                                                                                           |
| `docs/specs/rust-u2-host-skeleton.md` §6（第 118 行起）                      | 单一开关与回退矩阵描述                                                                                                          |

说明：启动失败时**当前代码并不回退**——`attachWorkspaceHost` 不捕获 `attachRustHostLocal` 的错误，错误上抛给远控 bridge（`webRemoteControlSharedHostAttachments.test.ts` “surfaces a Rust Host start error instead of falling back to the desktop host” 断言 utility Host 未被调用）。唯一的 Node Host 分支是“开关关闭”。窗口自身 host 仍是 Electron utility process（`desktopHostProcess.ts:737` `MessageChannelMain`、`:770` `registerHostProcess`），属 H2.3，不在 M5 删除清单内。

## 8. spec 漂移修正

| 文件                                            | 原表述                                              | 修正                                                                                                               |
| ----------------------------------------------- | --------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ |
| `rust-u2-host-skeleton.md` §3.2                 | “Gateway 入站（Renderer→Host RPC）256 条，触发背压” | 删除：真实 RPC 路径无入站队列，`host-gateway` 也没有入站队列                                                       |
| `rust-u2-host-skeleton.md` §3.2                 | Host 日志“丢弃最低级别并计数（warn 起不丢）”        | 改为“队满时任何级别都可能丢，按写者与进程计数，镜像 stderr”，写明理由；表格增加常量与构造点列、host-surface 出站行 |
| `rust-u2-host-skeleton.md` §3.4                 | “登记必须在请求到达时同步完成”未限定范围            | 增加适用范围：Gateway 只用于 host-surface；host-rpc 为每连接登记序 = 到达序                                        |
| `rust-u2-host-skeleton.md` §8 验收表            | “Gateway 到达序：并发请求登记顺序 = accept 顺序”    | 拆为 host-surface 与 host-rpc 两条证据                                                                             |
| `rust-u2-host-skeleton.md`“Host 日志出口接齐”段 | “满足 §3.2『丢弃最低级别并计数』”                   | 改为新口径，并指向 `dropped_logs()`                                                                                |
| `rust-u2-host-skeleton.md` dev 开关段           | “失败抛错由调用方回退 Node Host”                    | 改为抛分类 `RustHostStartError`、调用方不回退、意外退出受限重启                                                    |
| `rustHostManager.ts` 日志                       | `start failed, falling back to Node Host`           | 删除不实的“回退”，改为 `start failed (<code>)`、级别 error                                                         |

## 9. 未完成项与原因

1. **Windows / macOS（2026-10-19 更新）**：child 链路的 `taskkill /F` 分支已在官方 `rust-runtime-ci` run `39164847925` 的 Windows `test (windows-latest, 1/3)` 中执行并通过（`killed_child... ok`）；macOS 同一 run 的 host 分片也 passed。`unit-tests.yml` 现在构建 `escode-host` 与 `escode-cli-rust`，依赖真实 Host 的 desktop 用例不再被 `skipIf` 跳过。仍未执行：`RustHostStartError` 在 Windows 上 `ENOENT` 以外的 `errno` 分支（`rustHostResilience.test.ts` 只在 ubuntu 的 `unit-tests` 运行）。
2. **桌面窗口 Renderer 的“正在重启 / 已放弃”页面**：依赖窗口自身 host 接线（H2.3，本轮不做）。本轮 Renderer 可见面只有手机端（`bridge-degraded` / `workspace-bridge-error{desktop-disconnected}`），未新增文案，因此 `DESIGN.md` / i18n 无改动。
3. **手机端 replayable 恢复的端到端**：`handleHostAttachmentLost` → `bridge-degraded` 有单测，手机端重新 `workspace-bridge-open` 后接到重启后 Host 的全链路没有 E2E（H5.1 的 replayable E2E 仍缺）。UNVERIFIED。
4. **host-rpc 处理函数 panic**：独立任务里的处理函数 panic 时该请求没有响应（客户端等到自己的超时）。属 Host 级故障隔离的相邻缺口，未在 M2 范围内修改，记为后续项。
5. **Main 退出时未调用 `rustHostManager.stop()`**：`index.ts` 没有在应用退出路径收口 Rust Host（Host 随 stdin EOF 自行退出）。不影响 M2 判定，记为后续项。
6. `Instance::wait_ready` 仍以 20 ms 轮询等待 ready（既有实现，未改）。

## 10. 其它说明

- `apps/escode-cli-rust/Cargo.toml` 在 T2 提交时同时含他人的 zip 改动：用 `git hash-object -w` + `git update-index --cacheinfo` 只暂存 `[features]` 段，zip 行留在工作区，随后由并发工作流在 `6e130688` 提交。
- `pnpm typecheck` 的项目列表不含 `packages/desktop/tsconfig.main.json`（Main 代码）。对它单独跑 `tsc -p … --noEmit`：M2 改动前后均为 91 个错误，全部是既存（`autoUpdater.ts`、`browserGuestManager.ts`、`index.ts` 的 `MessageChannelMain` 全局名等），M2 文件无新增错误。
- 被测二进制：`escode-cli-rust` 于 05:37:01 由 `fa109b70` 构建；`escode-host` 由 `cargo test`/`check` 在同一 HEAD 构建；`test:escode-cli-rust` 开跑时 HEAD 为 `fa109b707dc264b82a9bf17488b643d1e1291ef7`（与被测 CLI 二进制同一提交）。

## 11. 复核补正（2026-10-19，实现侧）

### 11.1 Windows crash_isolation 的真实根因与修复

dev `380331a9` 的 `rust-runtime-ci` 首次三平台运行中，`test (windows-latest, 1/3)` 在 `crash_isolation` 的 child 用例失败：`A model request: Elapsed(())`（60 s 守卫超时，掩盖了真实错误）。

根因：`Link::Child` 用 `Command::env_clear` 隔离实例环境；Windows 上这会清掉 `SystemRoot`，而 Win32 的 socket / 系统证书库在缺少它时不可用（rust-lang/rust#114737）。child runtime 已报到 ready（存储与 stdio 正常），随后第一次模型请求无法建立 HTTP 连接，模型桩收不到请求。

修复：

- `link.rs` 在 Windows 上补回平台底座变量 `SystemRoot`、`SystemDrive`、`windir`、`ComSpec`、`PATHEXT`、`TEMP`、`TMP`、`PATH`；`RuntimeOptions.env` 的显式同名值优先。业务事实仍只来自 Main 的 `runtimeEnv`。
- `crash_isolation.rs` 的模型等待改为同时观察在飞请求结果与实例退出，后续同类失败会直接打印真实 `RuntimeClosed` / `ExitReport`，不再退化成 60 s 超时。

证据：

- 修复前：dev `380331a9` 的 `test (windows-latest, 1/3)` 失败（`A model request: Elapsed(())`）。
- 修复后定向运行（`host-crash-windows-check`，run `39163244358`）：`crash_isolation` 2 passed，child kill 用例实际执行（非 skip）。
- 修复后官方 `rust-runtime-ci`（run `39164847925`，三平台全量）：整条 run **success**。Windows `test (windows-latest, 1/3)`（含 `pnpm test:escode-host`）passed，日志中 `embedded_panic... ok` / `killed_child... ok`；Ubuntu 与 macOS 三个 shard、Windows 其余 shard 全部 passed。Windows shard 2 首次有一例与本改动无关的 `Node and Rust manage scheduled tasks the same way` 超时，重跑后 passed（flaky）。

### 11.2 RPC 层真实客户端错误形状

新增 `packages/desktop/test/rustHostRuntimeCrash.test.ts`：真实 `@escode/client` → host-rpc → child runtime。SIGSTOP 使转发请求停在在飞表，SIGKILL 后断言：

- `error instanceof Error`；
- `error.name === "Error"`（不是 `Fatal`）；
- `error.message === "ESCode agent transport closed: stdout_closed"`；
- `String(error)` 逐字一致；
- 同一崩溃发布 `unavailable` 生命周期事件。

该用例与既有 `rustHostRpc.test.ts` 在 `unit-tests` 的 affected job 中实际执行并 passed（run `39164040559`，`rustHostRuntimeCrash.test.ts` 454 ms）。`unit-tests.yml` 现在构建 `escode-host` 与 `escode-cli-rust` 二进制，依赖真实 Host 的 desktop 用例不再被 `skipIf` 静默跳过。

### 11.3 T3 独立构建

在独立 worktree（`8c8b67d6`）离线安装依赖后：

- `pnpm typecheck` exit 0；
- `rustHostResilience.test.ts` 7 passed / 1 skipped；
- `webRemoteControlSharedHostAttachments.test.ts` 5 passed；
- `webRemoteControlManagerExternalRelay.test.ts` 仅因该 worktree 未下载 Electron 二进制而在 import 阶段失败，与 T3 代码无关。

### 11.4 仍未完成 / 不在本轮范围

- **H2.1 / H2.2 整体**：Node Host 分支与 `ESCODE_RUST_HOST` 开关仍在，整体结论仍为 FAIL，等 M5 删除。
- **H2.7**：只覆盖 Host 内的实例隔离；窗口自身 host 仍是 Electron utility process（H2.3），手机重连到重启后 Host 的全链路仍无 E2E。
- **H1.3 / H1.4**：本轮未改语义；最终结论由独立复核人填写。
- **Windows 启动失败分类**：`RustHostStartError` 的 `ENOENT` 以外 errno 分支仍未在 Windows 执行；`rustHostResilience.test.ts` 只在 `unit-tests`（ubuntu）运行。本轮只验证了 Windows 上的 child 运行时链路与 `taskkill /F` 分支。
- `unit-tests` 的 Electron 依赖套件（`webRemoteControlTransport`、`desktopNetworkTelemetry` 等 10 个 suite）与 bootstrap suite 的失败在 dev 上已存在：前者是 `ELECTRON_SKIP_BINARY_DOWNLOAD=1` 与直接 `import electron` 冲突，后者是既存的 v4 语料/边界失败；两者均不由本轮改动引入。

## 12. 未完成项复核（2026-10-19）

下表按当前代码逐条复核 §9 与验收报告列出的 M2 剩余项；计数与路径为本日现场结果。

| 项               | 现状                                                                                                                                                                                                                                                                                                                                                                                                                                                                     | 归属        |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ----------- |
| H2.1 / H2.2 整体 | `ESCODE_RUST_HOST` 开关与 Node Host 回退仍在（`rustHostBridge.ts:81`、`webRemoteControlSharedHostAttachments.ts:168`、`index.ts:836`）；H2.2 只完成启动失败分类与显式报错前半。                                                                                                                                                                                                                                                                                          | M5 删除     |
| H2.3 窗口 host   | `desktopHostProcess.ts:36` 仍 import `utilityProcess`，`:756-770` 注册 `broadcastHub` / `taskRealtimeBus` / `registerHostProcess`；Rust Host 只服务 Web 远控的本地 workspace attachment。                                                                                                                                                                                                                                                                                | H2.3 / M5   |
| H2.6 Pending     | 46 channels / 539 members：334 Implemented、205 Pending；13 个 channel 全 Pending：`bots` 32、`plugin-ui-bridge` 12、`off-peak-task` 11、`conversation-share` 9、`plugin-sync` 7、`plugin-ui-app-tools` 4、`cloud-content` 3、`cua-pip-session` 3、`hooks` 3、`cua-permission` 2、`marketing-touch` 2、`plugin-ui-sampling` 2、`provider-provisioning-target` 1。`hooks` / `bots` / `off-peak` / `plugin-sync` 有明确 Renderer 调用点，切默认后会直接 `NotImplemented`。 | H2.6 / M5   |
| H2.7 作用域      | Host 内实例崩溃隔离已完成（`crash_isolation.rs` + Windows run `39164847925`）；窗口自身 Host（H2.3）与手机重连到重启后 Host 的 E2E 仍缺。                                                                                                                                                                                                                                                                                                                                | H2.3 / H5.1 |
| H3.4 / H3.5      | `apps/escode-host` 内没有 `workspace-hook-trust-v1`、`paste-attachments`、`cli/workflows`、`session-bindings` 的读取点；没有 `import_history` / `legacy_import`；默认 embedded Host 直接 `Store::open`，不做 TS 会话库导入。                                                                                                                                                                                                                                             | H3 / M5     |
| H5.1 / H5.3      | `packages/desktop/test/e2e` 内没有 `web-remote-replayable` 用例；`host-control` 无 `CronRun` / `OffPeakRun`；`desktopWindowLifecycle.ts:270` 在窗口关闭时销毁 host，窗口全关后调度不可用。                                                                                                                                                                                                                                                                               | H5 / M5     |
| H2.4 / H2.5      | 无图形会话、未构建安装包，进程树与安装包验收仍 UNVERIFIED（环境受限）。                                                                                                                                                                                                                                                                                                                                                                                                  | 验收环境    |
| Windows 启动失败 | `taskkill /F` 已验证；`RustHostStartError` 的 `ENOENT` 以外 errno 仍未在 Windows 执行（`rustHostResilience.test.ts` 只在 ubuntu `unit-tests`）。                                                                                                                                                                                                                                                                                                                         | 测试缺口    |
| M2 §9 技术债     | `connection.rs:194-198` 的 handler task 未捕获 panic，panic 时请求无响应；`index.ts` 无 `rustHostManager.stop()`；`instance.rs:310-319` 的 `wait_ready` 仍为 20 ms 轮询。                                                                                                                                                                                                                                                                                                | 后续        |

证据边界：H1.3 / H1.4 的最终结论仍由独立复核人填写；本表只记录实现侧事实，不替验收报告改判。
