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

判定（初稿）：既存/环境敏感失败。**复核修订**：该判定不成立，5 个失败均为可稳定复现的代码/测试缺陷，已在 §11 修复。

`cargo deny`（初稿）：本机未安装，UNVERIFIED。**复核修订**：已在本机运行并发现 advisories 失败，修复后通过，见 §11。CI 覆盖作业：`.github/workflows/rust-runtime-ci.yml` 的 `checks` job 使用 `EmbarkStudios/cargo-deny-action@v2`（manifest `apps/escode-cli-rust/Cargo.toml`）。2026-10-19 补记：已取得该作业的 CI 运行记录（run `39164847925`，三平台 `checks` 全绿；cargo-deny 只在 Linux 跑），见 §12。

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

| 命令                                            | 退出码 | 关键结果                                                                        |
| ----------------------------------------------- | ------ | ------------------------------------------------------------------------------- |
| `pnpm check:escode-cli-rust`                    | 0      | 边界 OK；fmt；clippy `-D warnings` 通过                                         |
| `pnpm check:escode-host`                        | 0      | 边界 OK；生成器 `--check`；46 channels / 539 members；clippy 通过               |
| `pnpm test:escode-cli-rust`（CI 口径）          | 0      | 复核后：Rust 61 二进制 464 passed；TS 411 项 399 pass / 0 fail / 12 skip（§11） |
| `pnpm test:escode-host`                         | 0      | 54 个测试二进制，723 passed / 0 failed；interop 1 passed                        |
| `pnpm typecheck && pnpm lint && pnpm fmt:check` | 0      | typecheck 通过；lint 0 error / 160 warning；fmt 全绿                            |
| `pnpm architecture:check --changed`             | 0      | violations 0 / new 0                                                            |
| T2 负向验证                                     | 1      | 边界脚本按预期失败；worktree 已删除                                             |
| `node scripts/bench-escode-app.mjs --self-test` | 0      | PASS（含 `parseCpuTime` 三种格式）                                              |

## 9. R 线逐项新结论

| ID                 | 结论 | 依据                                                                                                                                |
| ------------------ | ---- | ----------------------------------------------------------------------------------------------------------------------------------- |
| R3                 | PASS | 两个 workspace 并发真实 Write 工具，文件各落自身目录，收口 `running()` 为空                                                         |
| R5                 | PASS | tools 14 处、state owner 锁、wiki 解析全部经 Spawner；边界脚本负向验证 exit 1                                                       |
| R6                 | PASS | `MAX_REQUEST_BYTES` 超限在两条链路一致，真实 Engine 错误帧逐字节相等                                                                |
| R9                 | PASS | 完整入口 exit 0（Rust 464 / TS 399 pass、0 fail）；`cargo deny check` 本机 exit 0（advisories/bans/licenses/sources 全 ok）。见 §11 |
| R1/R2/R4/R7/R8/R10 | PASS | 本轮未改动对应实现；`check:escode-cli-rust` 与 embed/dual transport 套件回归通过                                                    |

R 线总判决：**ACCEPT**。验收报告 §2 的四条硬性条件（R5、R9、R3、R6）全部满足；R9 的两项剩余条件（5 个失败、`cargo deny`）已在 §11 收口。

## 10. 仍未解决

1. ~~5 个失败~~、~~`cargo deny`~~：已在 §11 收口。
2. ~~Host 无 `deny.toml`~~：已补，见 §11.4。
3. ~~`checks` 三平台矩阵未取得 CI 运行记录~~：已关闭（2026-10-19）。run `39164847925` 的 `checks`（ubuntu/windows/macos）与三平台 `test` 全部 success；详见 §12。
4. H2/H3/H5 本轮明确不做：唯一 Host 切换、Pending 业务域、TS 删除、replayable E2E、无窗口调度。
5. 验收报告中的其它非阻塞项（H1.3/H1.4/H1.8 的口径、四 locale readdir 证据、busy_timeout、H4 差异登记等）未在本轮处理。
6. 本机 Node 24.14.0 与 CI 期望 24.18.0 不一致；完整串行套件本机耗时约 31.4 分钟，CI 的 test 作业超时为 120 分钟。

## 11. 复核与收口（同日后续）

### 11.1 5 个失败的真实原因（提交 `42c80e06`）

§4 把它们记为“既存/环境敏感”，复核结论不成立：

| 用例                | 原因                                                                                                                                                     | 修复                                                           |
| ------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------- |
| plugins-install     | 回归：`87f7aa44` 错误类型化后分类只做 downcast，漏改 `Dependency not found` 产生点（仍是普通 `anyhow!`），落到兜底 `plugin_marketplace_invalid`          | 改抛 `Kind::DependencyMissing`，补 `plugin_install_entry` 单测 |
| plugins-list        | Node `readdirSync` 在 Unix 上经 libuv scandir 按名字字节序排序；Rust `read_dir` 为原始顺序（ext4 哈希序）。两侧 seed 副本 `ls -U` 顺序相同，排除环境因素 | 技能扫描复用 `plugin_describe_scan::entries`，补顺序单测       |
| model-io ×2         | MCP server 用 `BTreeMap` 合并，按名字排序使 `node_repl` 排到 `plugin:*` 前；TS 是对象 spread 插入序                                                      | 新增 `mcp_config_order::Ordered`（JS 键序语义），补单测        |
| mcp-request-context | 测试脆弱，失败在 **Node** 侧：模型桩取第一个 `mcp__` 工具，随包 usb 插件工具排在会话 server 前                                                           | 按 fixture server 名精确选取 `mcp__local_echo__`               |

spec 同步：`rust-plugins.md`（技能顺序）、`rust-browser-use.md`（MCP 合并键序；登记 `config.json` 的 `mcp.servers` 组内按名字序的已知差异）。

### 11.2 `cargo deny`（提交 `6e130688`）

本机使用 cargo-deny 0.20.2 官方 musl 预编译包（sha256 与发布页一致）。首次运行 advisories **FAILED**：

- `rustls 0.23.45`：RUSTSEC-2026-0346/0347/0348/0349 → 升级 0.23.46。
- `zip 8.6.0`：RUSTSEC-2026-0367/0368/0369 → 升级 9.0.5。代码未调用 `extract()`（插件解压为自有实现，按 `name_raw` 校验路径并拒绝符号链接），不可达，但策略检查必须清零。

zip 9 的条目名改为可失败解码，适配 `wiki/src/ingest/ooxml.rs`（嗅探与 `names()` 跳过解不出的名字；`open()` 的告警文案回退 lossy 文本）。Host 经 path 依赖共用 zip，同步更新 `apps/escode-host/Cargo.lock`。

```text
$ cargo deny --manifest-path apps/escode-cli-rust/Cargo.toml check
advisories ok, bans ok, licenses ok, sources ok
EXIT 0
```

### 11.3 验证记录

| 命令                                                                     | 代码状态                     | 退出码 | 结果                                                                        |
| ------------------------------------------------------------------------ | ---------------------------- | ------ | --------------------------------------------------------------------------- |
| `pnpm test:escode-cli-rust`（CI 口径，第 1 轮）                          | `42c80e06`                   | 0      | Rust 61 二进制 464 passed / 0 failed；TS 411 项 399 pass / 0 fail / 12 skip |
| `pnpm test:escode-cli-rust`（CI 口径，第 2 轮）                          | `42c80e06` + rustls/zip 升级 | 0      | 同上（TS 耗时约 31 分钟）                                                   |
| `pnpm test:escode-host`                                                  | `42c80e06` + 升级            | 0      | 54 二进制 723 passed / 0 failed                                             |
| `pnpm check:escode-cli-rust` / `check:escode-host`                       | `42c80e06` + 升级            | 0      | 边界、fmt、clippy `-D warnings` 通过                                        |
| `cargo deny check`（CLI manifest）                                       | `42c80e06` + 升级            | 0      | 四项全 ok                                                                   |
| `pnpm typecheck` / `lint` / `fmt:check` / `architecture:check --changed` | `42c80e06`                   | 0      | lint 0 error / 160 warning（既有）                                          |

覆盖范围说明：验证期间另一工作流在本分支提交了 `4f0de02c`、`818c040f`、`8c8b67d6`、`57ea5b0d`（H2 韧性：实例崩溃隔离、启动失败分类、Host 自动重启）。其中 `818c040f` 改动了 `escode-cli-rust`（`core/src/app/engine_requests.rs`、`src/embed.rs`、Cargo feature），**不在上表的套件覆盖范围内**；按要求本轮未在最终 HEAD 上重跑完整套件，这些提交的验证以其自身提交记录为准。

### 11.4 收尾与最终状态

- Host 工作区补 `apps/escode-host/deny.toml`（与 CLI 同一策略）与 `pnpm check:escode-host:deny`；CI 新增对 Host 的 `cargo-deny-action`。
- `checks` 作业改为 ubuntu/windows/macos 矩阵，推翻 §7 的“维持 ubuntu”决定（理由与代价见 `docs/specs/rust-ci.md`）；deny 仅在 Linux 跑。
- 分支说明：验证期间另一工作流把共享工作区切到 `m2/host-resilience`，§11.2 起的 M1 提交一度落在该分支、与 H2 提交交错。已在独立 worktree 中 cherry-pick 回 `m1/gates-and-r-line`。本分支只含 M1 改动，其 Rust/TS 代码与 §11.3 第 2 轮完整套件被测代码相同（之后只有报告、CI、spec 改动）；§11.3 关于 H2 提交不在覆盖范围的说明只适用于 m2 分支。

`m1/gates-and-r-line` 末端（干净 worktree）复核：

| 命令                                                                        | 退出码 | 结果      |
| --------------------------------------------------------------------------- | ------ | --------- |
| `check-escode-cli-rust-boundaries.mjs` / `check-escode-host-boundaries.mjs` | 0 / 0  | OK / OK   |
| `cargo fmt --check`（两个 workspace）                                       | 0      | —         |
| `cargo clippy --locked --all-targets -D warnings`（两个 workspace）         | 0      | —         |
| `cargo deny check`（CLI / Host）                                            | 0 / 0  | 四项全 ok |

最终状态：

| 项                      | 结论      | 说明                                                                                                                |
| ----------------------- | --------- | ------------------------------------------------------------------------------------------------------------------- |
| T1–T5、R 线四条硬性条件 | PASS      | R 线 **ACCEPT**                                                                                                     |
| T6 / H0“CI 三平台”      | PASS      | run `39164847925`：三平台 `checks` 与 `test` 全部 success（Windows shard 2 一次无关 cron 超时，重跑通过），见 §12   |
| m2 分支 Host 边界       | 不属于 M1 | `818c040f`（H2.7）使 `host-runtime/src/instance.rs` 490 行、`tests/crash_isolation.rs` 423 行超限，由 H2 工作流拆分 |

### 11.5 推送门禁（pre-push）暴露的既存单测问题

为取得 H0“CI 三平台”的运行记录需推送本分支；pre-push（lint → architecture → 受影响单测）在本分支相对 main 的 182 个提交范围内选中全量单测，拦下推送。处理如下（均在 M1 基线 `33cf6b34` 上已同样失败，A/B 失败集合逐条一致，非 M1 引入）：

1. **22 个失败文件**（提交 `63acf75a`、`48976eca`）：serial ×11 用 `node:test` 编写被 vitest 收集；e2eReporting ×3 与 serialAgentChain 缺 CI 构建产物（`unit-tests.yml` 补 `@escode/serial-plugin` 与 `@escode/e2e-report` 的 `build:node`）；`tsupConfig`、`desktopE2EContainerScript`、`webRemoteControlTaskIndexLayout` 为 `4cb7b20d` 全仓格式化后的断言漂移或打包规则过严；`remoteDeploy` 读宿主机 profile；`providerRegistry` mock 缺 `runtime/capabilities`；`rustHostBridge` 缺二进制未跳过。另修复一处产品缺陷：subagent 配置目录被删后立即重建时，ext4 复用 inode 使监听失效（身份加入 `birthtimeMs`，spec `rust-u2-subagents.md` 同步）。
2. **worker OOM**：上述修复后全部用例通过，但 vitest 仍报 1 个 unhandled error（threads 池为堆 OOM、forks 池为 worker 意外退出），pre-push exit 1。定位到 `packages/ui/test/useDraftConfigControl.test.ts`：mock 了转导出模块而非 `workspacePrepareRpc.js`（mock 不生效），且 `useESCodeSessionService` mock 每次渲染返回新对象，作为 SWR key 的 service 身份每次变化 → 无限重取与重渲染，worker 堆约 79 秒涨到 4 GB。修正两处 mock 后整文件 4 秒跑完。
3. **7 个待产品裁决的用例**：OOM 修复后同文件暴露 7 个断言失败（SHARE31 的 `mode` 组、slashCommands 保留、Registry revision、Session 只初始化一次、新 Root 继承权限、新任务转移 Draft、PA158 授权消费）。在 `13c5a1a2`（内容等同 v3.15.1 导入）上套用同样 mock 修复后失败完全相同，说明自导入起即与实现不符、一直被 OOM 掩盖，不是后续提交的回归。断言的是 composer 草稿产品语义，以测试还是实现为准需产品裁决；按决定登记跳过（`it.skip` / `skip(KNOWN_DRAFT_SEMANTICS_GAP)`，原因随报告输出），不改断言、不改实现，另行跟进。

## 12. CI 三平台补记（2026-10-19）

M1 收尾时 H0“CI 三平台”因分支未推送而标为 UNVERIFIED。2026-10-19 已在包含 M1/M2 合并结果与本轮 Windows crash 修复的 `fix/host-runtime-crash-ci` 上取得 `rust-runtime-ci` 全量运行记录（[run 39164847925](https://github.com/sekfung/ESCode/actions/runs/39164847925)）：

| 作业                                 | 结果                                                                                                                            |
| ------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------- |
| `checks`（ubuntu / windows / macos） | 全部 success                                                                                                                    |
| `test`（三平台 × 3 shard）           | 全部 success；Windows shard 2 首次 `Node and Rust manage scheduled tasks the same way` 超时，重跑通过（与本轮改动无关的 flake） |
| `bench`（三平台）                    | 全部 success                                                                                                                    |

因此 H0 的“CI 三平台”子条件关闭。仍未变的是 §7 记录的单独事项：parity 作业仍是 `continue-on-error: true`（非阻塞），不属于“三平台门禁”这一条；若要把整个 H0 判 PASS，需另行裁决 parity 是否必须阻塞。

证据边界：该 run 验证的是 M1/M2 合并后、再叠加本轮 Windows crash 修复的树，不是 M1 分支末端 `m1/gates-and-r-line` 的逐提交记录；M1 自身的 Rust/TS 套件与边界检查仍以 §8 / §11.3 的记录为准。
