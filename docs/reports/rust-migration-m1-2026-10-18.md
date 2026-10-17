# Rust 迁移 M1 报告：门禁变绿 + R 线收口（2026-10-18）

## 1. 基线与范围

| 项          | 值                                                                                                                 |
| ----------- | ------------------------------------------------------------------------------------------------------------------ |
| 分支        | `m1/gates-and-r-line`（从 `dev` `33cf6b34` 新建）                                                                  |
| 平台        | Linux x86_64（VMware，7.0.0-34-generic）                                                                           |
| Rust        | rustc 1.95.0（与 `rust-toolchain.toml` 一致）                                                                      |
| Node / pnpm | v24.14.0 / 10.33.2（CI 期望 Node 24.18.0，本机有 engine 警告）                                                     |
| 开工工作区  | `git status --short` 为空                                                                                          |
| freshness   | `node scripts/check-workspace-freshness.mjs` exit 0：dev 与 origin/dev 同步，相对 origin/main ahead 164 / behind 0 |

本轮只做 M1（T1–T6）：不改 H2 唯一 Host 切换、不补 Pending 业务域、不删 TS 代码。未触碰其它工作流的工作树 `/tmp/escode-head-check`。

## 2. T1：H0 门禁变绿

| 项                                              | 结果                                                                                                                |
| ----------------------------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| `domains_task.rs`                               | 401 → 352 行：`getTaskModelSelection` 处理器移到 `domains_task_query.rs`（同一 `ReadContext` 语义，行为不变）       |
| `rpc_task_remaining.rs`                         | 405 → 319 行：`Fixture`/`fixture()`/`object()`/`call_task()` 移到 `tests/support/task_fixture.rs`，测试体与断言原样 |
| `docs/specs/bots-v4-stream.md`                  | `oxfmt` 格式化；`pnpm exec oxfmt --check` exit 0                                                                    |
| `node scripts/check-escode-host-boundaries.mjs` | exit 0，`Rust Host boundaries: OK`                                                                                  |

`domains_task.rs` 的 `path_for_snapshot` 复用与 `domains_task_tests.rs` 的 `Value` 导入在首次编译时修正（见提交 `a1e92ec3`）；边界脚本单独通过不代表 host workspace 编译通过，本轮已补跑。

## 3. T2：R5 spawn 纪律收口 + 机器守护

### 3.1 迁移清单

全部改经 `escode_cli_core_api::scope::Spawner::current()`：

- tools 14 处：`tool_process.rs`（bash 输出捕获）、`checkpoint_blobs.rs`（父目录 fsync）、`bash_image.rs`（图片预算）、`extension_plugins.rs`（官方插件 seed）、`plugin_describe.rs`、`plugin_install.rs`（安装闭包 + reseed）、`plugin_uninstall.rs`（reseed）、`plugin_validate.rs`（3 处）、`read_image.rs`（压缩）、`read_pdf.rs`（逐页压缩）、`tool_search.rs`。
- `state/src/storage.rs`：owner 锁获取（原 `:131`）。
- `wiki/src/pipeline.rs`：解析阻塞任务（原 `:156`）。

保留的 §3.1 例外（按文件 + 理由登记）：

- `core-api/src/scope.rs`：Spawner 实现本身。
- `state/src/storage.rs`：`Store::open` 的 SQLite actor（随最后一个 `Store` 克隆 drop 结束）。
- Link 传输线程与 file_log 单例使用 `std::thread`，不在 tokio spawn 规则范围。

### 3.2 边界脚本

`scripts/check-escode-cli-rust-boundaries.mjs` 新增：

- 非测试代码禁止 `tokio::spawn(` / `tokio::task::spawn_blocking(`；`tests/` 与 `*_tests.rs` 以及 `#[cfg(test)]` 段跳过。
- 例外以 `Map<文件, 理由>` 登记（`core-api/src/scope.rs`、`state/src/storage.rs`），不按行号。

### 3.3 负向验证

独立 worktree（detached `HEAD`，`/tmp/escode-m1-neg`）在 `crates/model/src/lib.rs` 追加：

```rust
#[allow(dead_code)]
fn m1_spawn_probe() { tokio::spawn(async {}); }
```

```text
$ node scripts/check-escode-cli-rust-boundaries.mjs
model/src/lib.rs: spawns outside Spawner (use Spawner::current(); exceptions are registered by file)
EXIT:1
```

验证后 `git worktree remove --force /tmp/escode-m1-neg`，`git worktree list` 只剩主工作区与其它工作流的 `/tmp/escode-head-check`。

### 3.4 顺带修复：Spawner 账目竞态

完整 `embed_memory_link` 首次运行暴露 `running()` 残留 `workspace-lock`：极快任务在 `register` 前完成，`TaskGuard` 注销落空后条目被重新插入。改为**先登记占位条目、spawn 后再挂 abort handle**（条目已被注销时不再插回），并补 256 个快速阻塞任务的回归用例；`scope::tests` 7 passed。

### 3.5 spec

- `docs/specs/rust-u1-runtime-lib.md` §3.5 增加 spawn 规则与文件级例外。
- 第 80 行迁移点改为“tools 20 处（含本轮补齐的 14 处裸 `spawn_blocking`）… 收口后只剩 scope.rs 与 Store::open 两处”。

## 4. T3：R9 恢复完整套件入口

修复的 TS 类型错误：

1. `escode-cli-rust-fixture.ts`：导出 `Message`。
2. `escode-cli-rust-topic-import.test.ts`：先按 `info.role === "user"` 收窄再读 `source`，不用 `any`/`as unknown as`。
3. `escode-cli-rust-queue-promotion-frame.test.ts`（`33cf6b34` 新增，验收报告后出现的既存错误）：`findLast` 用显式 `StateUpdatedDelta` 谓词收窄，消除 `patch` 联合访问与隐式 `any`。

`node node_modules/typescript/bin/tsc -p packages/services/tests/tsconfig.escode-cli-rust.json` exit 0。

完整套件按 CI 口径运行（`ESCODE_TEST_SERIAL=1`、`ESCODE_TEST_REQUEST_MS=60000`、`ESCODE_TEST_EXIT_MS=15000`）：结果见 §8。

完整套件还暴露一处加速卡删除后的 parity 漂移：Rust `finish_rows` 仍写 `modelDurationMs/toolDurationMs/otherDurationMs`，而 Node 在 2026-10-15 套餐族删除后不再写。已删除旧拆分并更新 `rust-v3.15.1-drift.md`；`escode-cli-rust-differential.test.ts` 14/14 通过。

完整套件结果（CI 口径，耗时约 31.4 分钟）：

- Rust：61 个测试二进制，461 passed / 0 failed。
- TS App 集成：411 项，394 passed / 5 failed（另有 skip），**exit 1**。

5 个失败均不在本轮改动路径（M1 只改 spawn 包装、行计时字段、测试与文档），且重建 `escode.cjs` 后复跑仍复现，排除 Node 产物陈旧：

1. `escode-cli-rust-mcp-request-context.test.ts`：`ENOENT .../mcp-normal-calls`。模型选中了另一个 `mcp__` 官方插件工具，fixture 的 `local.echo` 未被调用；依赖官方插件工具顺序。
2. `escode-cli-rust-model-io.test.ts`（2 例）：`toolNames` 中 `mcp__node_repl__js` 的位置与 Node 不同；同为官方插件发现顺序。
3. `escode-cli-rust-plugins-install.test.ts`：依赖缺失返回 `plugin_marketplace_invalid`，Node 为 `plugin_dependency_missing`；分类函数 `error_diagnostic` 本轮未改。
4. `escode-cli-rust-plugins-list.test.ts`：同一插件 skill 集合相同、顺序不同（Rust seed 复制后的 readdir 顺序 vs Node 读原目录）。

判定：既存/环境敏感失败，需单独开单（插件发现顺序与依赖诊断分类），不混入 M1 的 R 线收口结论。

`cargo deny`：本机 `cargo deny --version` exit 101（未安装），**UNVERIFIED**。CI 覆盖作业：`.github/workflows/rust-runtime-ci.yml` 的 `checks` job 使用 `EmbarkStudios/cargo-deny-action@v2`（manifest `apps/escode-cli-rust/Cargo.toml`）。本轮未取得该作业的 CI 运行记录。

## 5. T4：R3 / R6 测试缺口

### R3：两个 workspace 并发写文件

`apps/escode-cli-rust/tests/embed_memory_link.rs::concurrent_workspaces_write_tools_stay_in_their_own_directories`：

- 两个 `embed::spawn` 实例各自独立 workspace/data/home；每个实例配一个本地 SSE 模型桩，首个主回合返回 `Write(only-a.txt|only-b.txt)`，工具结果后返回 stop；标题请求（`tools` 为空）单独应答。
- 测试代 Host 应答 `session/requestRuntimePreferences`，否则首轮不会启动。
- 断言：两文件各自落在本 workspace 且内容正确；对方 workspace 不存在对方文件；EOF 收口后两边 `tasks.running()` 均为空。
- 结果：`cargo test --test embed_memory_link` 6 passed。

### R6：`MAX_REQUEST_BYTES` 超限

`apps/escode-cli-rust/tests/dual_transport.rs` 新增两例：

- `both_links_flag_requests_over_max_bytes`：>1 MiB 行在 StdioLink（进程内 pipe）与 MemoryLink 上都只产出 `too-large` 终态。
- `both_links_emit_the_same_oversize_error_frame`：同一超限输入经真实 `Engine`，两条链路输出的 `Request exceeds size limit` 错误帧逐字节相等。
- 结果：`cargo test --test dual_transport` 5 passed。

## 6. T5：H1 小缺口

| 项       | 结果                                                                                                                                                                                                                                     |
| -------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| H1.5     | `connection.rs` 新增 2 例：>8 MiB 触发 saturated 边沿、写回 <2 MiB 触发 drained、>64 MiB 触发硬上限通知；`cargo test -p escode-host-rpc outbound_` 2 passed                                                                              |
| H1.6     | `tests/host_entry.rs` 的 `shutdown()` 等待 `Stopped` 并断言 `residual` 为空，再断言退出码 0；3 passed                                                                                                                                    |
| 陈旧断言 | `rustHostRpc.test.ts` 的 `serial`/`escode-task.createTask` 已迁移：改用 `hooks.loadHooks`（未注册 channel）与 `escode-agent.readSessionDebug`（Pending 方法）；`pnpm exec vitest run packages/desktop/test/rustHostRpc.test.ts` 1 passed |
| bench    | `parseCpuTime` 支持 `[[DD-]hh:]mm:ss`；`--self-test` 覆盖 `01:02`、`1:02:03`、`2-12:46:17`；`node scripts/bench-escode-app.mjs --self-test` exit 0（PASS）                                                                               |

另：首次 `pnpm test:escode-host` 的 `contract.rs` 因 `host-proto` 陈旧增量产物报 `file.stat` 状态不一致；强制重编译后 3 passed，非源码问题。

## 7. T6：CI

- `.github/workflows/rust-runtime-ci.yml` 的 push 分支加入 `dev`。
- `checks` 维持 ubuntu-only：边界/生成器/typecheck/lint/fmt 与宿主 OS 无关；`test` 作业已在三平台编译并执行两个 workspace 的全部 target，覆盖平台 `cfg` 分支。扩成三平台会把 45 分钟作业乘以三，主要增量是 clippy 在 Windows/macOS cfg 下的 lint，留待唯一 Host 切换前评估；理由同步写入 `docs/specs/rust-ci.md`。
- parity 的 `continue-on-error: true` **本轮未改**。

## 8. 完成判定

| 命令                                            | 退出码 | 关键结果                                                                    |
| ----------------------------------------------- | ------ | --------------------------------------------------------------------------- |
| `pnpm check:escode-cli-rust`                    | 0      | 边界 OK；fmt；clippy `-D warnings` 通过                                     |
| `pnpm check:escode-host`                        | 0      | 边界 OK；生成器 `--check`；46 channels / 539 members；clippy 通过           |
| `pnpm test:escode-cli-rust`（CI 口径）          | 1      | Rust 61 二进制 461 passed；TS 411 项 394 pass / 5 fail（5 个既存失败见 §4） |
| `pnpm test:escode-host`                         | 0      | 54 个测试二进制，723 passed / 0 failed；interop 1 passed                    |
| `pnpm typecheck && pnpm lint && pnpm fmt:check` | 0      | typecheck 通过；lint 0 error / 160 warning；fmt 全绿                        |
| `pnpm architecture:check --changed`             | 0      | violations 0 / new 0                                                        |
| T2 负向验证                                     | 1      | 边界脚本按预期失败；worktree 已删除                                         |
| `node scripts/bench-escode-app.mjs --self-test` | 0      | PASS（含 `parseCpuTime` 三种格式）                                          |

## 9. R 线逐项新结论

| ID                 | 结论    | 依据                                                                                                                                              |
| ------------------ | ------- | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| R3                 | PASS    | 两个 workspace 并发真实 Write 工具，文件各落自身目录，收口 `running()` 为空                                                                       |
| R5                 | PASS    | tools 14 处、state owner 锁、wiki 解析全部经 Spawner；边界脚本负向验证 exit 1                                                                     |
| R6                 | PASS    | `MAX_REQUEST_BYTES` 超限在两条链路一致，真实 Engine 错误帧逐字节相等                                                                              |
| R9                 | PARTIAL | 套件入口已恢复：tsc 通过、cargo 461 例全过、TS 411 项能跑完；但 5 个既存失败使完整入口 exit 1。`cargo deny` 本机 UNVERIFIED，CI `checks` 作业覆盖 |
| R1/R2/R4/R7/R8/R10 | PASS    | 本轮未改动对应实现；`check:escode-cli-rust` 与 embed/dual transport 套件回归通过                                                                  |

R 线总判决：**ACCEPT WITH CONDITIONS**。R3/R5/R6 的硬性条件已满足；R9 的“完整入口 exit 0”未达成，剩余条件收敛为：

1. 修复 §4 列出的 5 个既存失败（插件发现顺序、MCP 工具选择、依赖诊断分类、model-io 工具顺序），使 `pnpm test:escode-cli-rust` exit 0；
2. 取得 `cargo deny` 的 CI 运行记录（本机未安装）。

这两项均不在 M1 的 T1–T6 改动范围内，未混入本轮结论。

## 10. 仍未解决

1. `pnpm test:escode-cli-rust` 的 5 个既存失败（见 §4）：mcp-request-context、model-io ×2、plugins-install、plugins-list。均已定位到未改动路径，需单独修复。
2. `cargo deny` 本机未执行（UNVERIFIED），CI 覆盖但本轮未取得运行记录。
3. `checks` 作业仍只在 ubuntu（理由见 §7）；H0 的“CI 三平台”仍是 PARTIAL。
4. H2/H3/H5 本轮明确不做：唯一 Host 切换、Pending 业务域、TS 删除、replayable E2E、无窗口调度。
5. 验收报告中的其它非阻塞项（H1.3/H1.4/H1.8 的口径、四 locale readdir 证据、busy_timeout、H4 差异登记等）未在本轮处理。
6. 本机 Node 24.14.0 与 CI 期望 24.18.0 不一致；完整串行套件本机耗时约 31.4 分钟，CI 的 test 作业超时为 120 分钟。
