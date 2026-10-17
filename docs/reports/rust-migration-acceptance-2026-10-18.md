# Rust 迁移独立验收报告（2026-10-18）

> 后续：见 [M1 报告](./rust-migration-m1-2026-10-18.md)（2026-10-18，门禁变绿 + R 线收口）。

## 1. 基线与范围

| 项                | 值                                                                                |
| ----------------- | --------------------------------------------------------------------------------- |
| 分支              | `dev`                                                                             |
| 审计开始时 commit | `2a18ec7f64f92fe9263501a0bd289e5ef667a985`（2026-10-18 00:39:26 +0800）           |
| 审计期间 HEAD     | `89eee7c4`（01:18:17）→ `c15a22aa`（01:29:36，当前）                              |
| 平台              | Linux x86_64（VMware，7.0.0-34-generic）                                          |
| Rust              | rustc 1.95.0（2026-04-14），与 `rust-toolchain.toml` 的 `channel = "1.95.0"` 一致 |
| Node / pnpm       | v24.14.0 / 10.33.2（CI 期望 Node 24.18.0，本机有 engine 警告）                    |
| 审计结束时工作区  | `git status --short` 为空（干净）                                                 |

说明：审计期间有另一条并发工作流把原本未提交的 bots v4 改动连续提交为 `89eee7c4`、`c15a22aa`。这两个提交只改 TS bots 代码、测试与文档，未改 `apps/escode-cli-rust` 与 `apps/escode-host` 的 Rust 代码；R/H 的 Rust 证据对当前 HEAD 仍成立。另一个工作树 `/tmp/escode-head-check`（`1f885ca4`）属于该并发工作流，本报告未触碰、未删除；本报告自己的负向验证工作树 `/tmp/escode-acc-wt` 已删除。

`node scripts/check-workspace-freshness.mjs` 因 `git fetch origin` 网络失败退出 1（TLS handshake），属环境限制，不是仓库状态问题。

### 跑过的命令与结果

| 命令                                                                                                      | 退出码 | 关键结果                                                                                                     |
| --------------------------------------------------------------------------------------------------------- | ------ | ------------------------------------------------------------------------------------------------------------ |
| `node scripts/check-escode-cli-rust-boundaries.mjs`                                                       | 0      | `Rust workspace boundaries: OK`                                                                              |
| `pnpm check:escode-cli-rust`                                                                              | 0      | 边界 + `cargo fmt --check` + clippy `-D warnings` 全过                                                       |
| `node scripts/check-escode-host-boundaries.mjs`                                                           | 1      | `host-rpc/src/domains_task.rs: exceeds 400 lines`；`host-rpc/tests/rpc_task_remaining.rs: exceeds 400 lines` |
| `cargo fmt --manifest-path apps/escode-host/Cargo.toml --all --check`                                     | 0      | 通过                                                                                                         |
| `cargo clippy --manifest-path apps/escode-host/Cargo.toml --workspace --all-targets -- -D warnings`       | 0      | 通过                                                                                                         |
| `cargo test --manifest-path apps/escode-host/Cargo.toml --workspace`                                      | 0      | 54 个测试二进制，721 passed / 0 failed                                                                       |
| `node scripts/test-escode-host-interop.mjs`                                                               | 0      | 双 Host settings 交替写读 1 passed                                                                           |
| `pnpm typecheck`                                                                                          | 0      | 通过                                                                                                         |
| `pnpm lint`                                                                                               | 0      | 0 error / 160 warning                                                                                        |
| `pnpm fmt:check`                                                                                          | 1      | 仅 `docs/specs/bots-v4-stream.md` 格式不符（当前 HEAD 已提交该文件）                                         |
| `pnpm architecture:check --changed`                                                                       | 0      | violations 0 / new 0                                                                                         |
| `cargo test --locked --workspace --manifest-path apps/escode-cli-rust/Cargo.toml`                         | 0      | 59 个测试二进制，455 passed / 0 failed                                                                       |
| `pnpm test:escode-cli-rust`                                                                               | 1      | 在 `tsc -p packages/services/tests/tsconfig.escode-cli-rust.json` 步骤失败，未进入 cargo/vitest；详见 §11    |
| `pnpm check:escode-cli-rust:deny`                                                                         | 101    | 本机未安装 `cargo-deny`（`no such command: deny`），UNVERIFIED                                               |
| 单测 `in_process_control_round_trip_latency_baseline --nocapture`                                         | 0      | p50 0.175 ms / p95 0.274 ms（debug，n=50）                                                                   |
| `npx vitest run packages/desktop/test/rustHostRpc.test.ts rustHostAgentV4.test.ts rustHostBridge.test.ts` | 1      | 7 passed / 1 failed（`rustHostRpc.test.ts:62` 的 serial 陈旧断言）                                           |
| `node scripts/bench-escode-app.mjs --self-test`                                                           | 1      | `Invalid CPU time: 2-12:46:17`（见 §10）                                                                     |
| `node scripts/generate-escode-host-service-inventory.mjs --check`                                         | 0      | 46 channels / 539 members                                                                                    |

## 2. 总判决

| 线                | 判决                       | 理由                                                                                                                                                                                                                                           |
| ----------------- | -------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| R：runtime 库化   | **ACCEPT WITH CONDITIONS** | R1/R2/R4/R7/R8/R10 通过；R3 缺“工具写文件落各自目录”的证据；R5 存在多处绕过 Spawner 的 `spawn_blocking`；R6 双传输未覆盖 `MAX_REQUEST_BYTES` 超限；R9 被既存 TS 类型错误挡在套件入口（Rust 侧 455 例全过）。含 2 项 FAIL，条件为硬性（见下）。 |
| H：Rust Host 迁移 | **REJECT**                 | H0 门禁红（400 行边界 + `pnpm fmt:check`）；H2.1/H2.2/H2.3/H2.6/H2.7 均失败，唯一 Host 目标不成立；H3.5 无 Host 首启迁移；H5.1/H5.3 未达标。                                                                                                   |

计数（UNVERIFIED 不计入通过率）：

- R：PASS 6 / PARTIAL 2 / FAIL 2，共 10 项，通过率 60%。
- H：PASS 11 / PARTIAL 14 / FAIL 11 / UNVERIFIED 2，共 38 项（H0 7 + H1 10 + H2 7 + H3 5 + H4 6 域 + H5 3），通过率 11/36 ≈ 31%。
  - 分组：H0 4/1/2/0、H1 6/3/1/0、H2 0/0/5/2、H3 1/3/1/0、H4 0/6/0/0、H5 0/1/2/0（PASS/PARTIAL/FAIL/UNVERIFIED）。
  - 修订说明：初稿写作 9/10/10/2 共 31 项，与逐项表不符；复核时按逐项表重数，并把 H1.3、H1.6 由 PASS 下调为 PARTIAL（理由见各行备注）。

R 线 `ACCEPT WITH CONDITIONS` 的硬性条件（未满足前不得视为 ACCEPT，也不得作为 Host 嵌入的前置已完成项）：

1. R5：14 处 tools、`state/src/storage.rs:131`（owner 锁获取，非 §3.1 例外）与 `wiki/src/pipeline.rs:156` 全部改经 `Spawner`，**并且**在 `check-escode-cli-rust-boundaries.mjs` 加入裸 `tokio::spawn` / `tokio::task::spawn_blocking` 规则（豁免 `core-api/src/scope.rs` 与 §3.1 三个例外），用 §5-2 的负向验证证明会拦。
2. R9：修复 §11-1 两个既存 TS 类型错误，`pnpm test:escode-cli-rust` 完整入口 exit 0；`cargo deny` 由本机或 CI 运行记录证明。
3. R3：补“两个 workspace 各自运行写文件工具，文件落在各自目录”的用例。
4. R6：双传输补 `MAX_REQUEST_BYTES` 超限用例。

## 3. R 线逐项

| ID  | 结论    | 证据                                                                                                                                                                                                                                                                                                                                                                                                            | 备注                                                                                                                                                                                                                             |
| --- | ------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| R1  | PASS    | `pnpm check:escode-cli-rust` exit 0；独立 `rg` 仅命中豁免文件 `core-api/src/env.rs` 与测试代码；负向验证见 §5-1                                                                                                                                                                                                                                                                                                 | 边界脚本同时覆盖 `use std::env;` 后的裸 `env::var` 形式                                                                                                                                                                          |
| R2  | PASS    | `tests/embed_memory_link.rs:228-263`：进程 cwd 为仓库根、进程 env 无 `ESCODE_CLI_RUST_DATA_DIR`，实例 env 注入相对路径，断言 `databaseId` 由 `workspace/relative-data` 派生                                                                                                                                                                                                                                     | 证明实例使用 `RuntimeOptions` 而非进程事实                                                                                                                                                                                       |
| R3  | PARTIAL | 同文件 `:163-200`：两实例不同 data_dir、会话互不可见、A 收口后 B 继续服务                                                                                                                                                                                                                                                                                                                                       | 未运行工具，也未断言工具产生的文件落在各自 workspace；缺该项即 PARTIAL（任务口径）                                                                                                                                               |
| R4  | PASS    | `:204-226`：同 options 双 spawn 第二个以 `WorkspaceOwned` 失败，关闭后重 spawn 成功；`crates/state/src/storage.rs:86-99` 先注销登记表条目再 `file.take()` 释放锁                                                                                                                                                                                                                                                | 顺序符合 §3.6                                                                                                                                                                                                                    |
| R5  | FAIL    | 非测试代码仍直接 `tokio::task::spawn_blocking`：`state/src/storage.rs:131`；`tools/src/tool_process.rs:29`、`read_image.rs:39`、`read_pdf.rs:240`、`tool_search.rs:24`、`checkpoint_blobs.rs:71`、`bash_image.rs:47`、`plugin_install.rs:154,240`、`plugin_validate.rs:45,155,232`、`plugin_describe.rs:67`、`plugin_uninstall.rs:269`、`extension_plugins.rs:63`（tools 共 14 处）；`wiki/src/pipeline.rs:156` | 这些调用在工具执行路径内，绕过 Spawner 的 `running()` 与 panic 上报；`Store::open` 的 actor（`storage.rs:184`）属 §3.1 例外，不计；`net`/`model`/`plugin_skill_files` 的调用已经 `Spawner`，不计。负向验证证明无机器守护（§5-2） |
| R6  | PARTIAL | `tests/dual_transport.rs` 3 例：同一字节流的 Input 序列、输出帧字节、EOF/关闭语义；覆盖无效 JSON、空白行、无换行尾行                                                                                                                                                                                                                                                                                            | 未覆盖 `MAX_REQUEST_BYTES`（1 MiB）超限；该路径只在 `stdio_input.rs:86-87` 实现，双传输没有对应断言                                                                                                                              |
| R7  | PASS    | `src/embed.rs:1-10,115-146` 无 `process::exit`/信号监听；`WorkspaceOwned` 以 `anyhow::Error` 返回，`:216-219` 测试 downcast                                                                                                                                                                                                                                                                                     | 收口走 `serve` + `drain`                                                                                                                                                                                                         |
| R8  | PASS    | 本轮 `p50=0.175 ms / p95=0.274 ms`（debug，n=50，MemoryLink NDJSON）；spec §6 记录为 0.250/0.334                                                                                                                                                                                                                                                                                                                | 阶段 1 只要求基线，≤0.1 ms 是阶段 4 目标                                                                                                                                                                                         |
| R9  | FAIL    | Rust 侧：`cargo test --workspace` 455 passed / 0 failed。完整入口 `pnpm test:escode-cli-rust` exit 1，未进入 cargo/vitest；`cargo deny` 本机缺命令                                                                                                                                                                                                                                                              | 失败是既存 TS 类型错误（§11-1），不是本轮 Rust 回归；但 R9 的“既有套件不回退”无法按完整入口证明                                                                                                                                  |
| R10 | PASS    | `host/src/net_config.rs:13-17,74,82` 进程级 `OnceLock` + `install`（`src/runtime.rs:39`）；`host_broker.rs`/`browser_broker.rs` 进程级 socket/token；`host/src/file_log.rs:43` 静态 `WRITER`                                                                                                                                                                                                                    | 与 spec §3.2/§7 登记的债务一致，未被误标为已实例化                                                                                                                                                                               |

## 4. H 线逐项

### H0 通用门禁

| 项                                  | 结论    | 证据                                                                                                                                                                                   | 备注                                                                                                                        |
| ----------------------------------- | ------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------- |
| `pnpm check:escode-host`            | FAIL    | exit 1；`domains_task.rs` 401 行、`rpc_task_remaining.rs` 405 行                                                                                                                       | 边界脚本 `scripts/check-escode-host-boundaries.mjs:24` 的行数规则在 `:25` 的 `tests/` 跳过之前执行，测试文件也受 400 行限制 |
| `pnpm test:escode-host`             | PASS    | 721 passed / 0 failed；interop 1 passed                                                                                                                                                | 721 含 `host_entry` 2 例、`instances` 7 例（含 child）、`rpc_surface` 3 例等                                                |
| `pnpm typecheck`                    | PASS    | exit 0                                                                                                                                                                                 | —                                                                                                                           |
| `pnpm lint`                         | PASS    | 0 error                                                                                                                                                                                | 160 条 warning，未作为失败                                                                                                  |
| `pnpm fmt:check`                    | FAIL    | `docs/specs/bots-v4-stream.md`                                                                                                                                                         | 该文件由并发提交 `c15a22aa` 引入；审计开始时是未跟踪 WIP，同样红                                                            |
| `pnpm architecture:check --changed` | PASS    | violations 0                                                                                                                                                                           | —                                                                                                                           |
| CI 三平台                           | PARTIAL | `.github/workflows/rust-runtime-ci.yml:33-35` checks 作业仅 ubuntu；`:70-75` test 作业三平台；`:148` parity `continue-on-error: true`；`:4-6` push 只覆盖 `main`/`feat/**`，不含 `dev` | “门禁在 CI 三平台都跑”不成立：check/typecheck/lint/architecture 只在 ubuntu，test 才三平台；parity 仍非阻塞                 |

### H1 骨架

| ID    | 结论    | 证据                                                                                                                                                                                                  | 备注                                                                                                                                                                                                                                                                                                      |
| ----- | ------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| H1.1  | PASS    | `crates/host-rpc/tests/contract.rs` 3 例随 721 全过；`generate-escode-host-service-inventory.mjs --check` exit 0；双向负向见 §5-4                                                                     | 翻 Pending 为 Implemented 与删已实现登记都被拦                                                                                                                                                                                                                                                            |
| H1.2  | PASS    | `session.rs:199-212` 表外方法/通道回 `NotImplemented`；`packages/desktop/test/rustHostRpc.test.ts:61-63,416,483-488` 真实客户端收到错误而非挂起                                                       | 错误文案含已实现清单                                                                                                                                                                                                                                                                                      |
| H1.3  | PARTIAL | `host-gateway/src/lib.rs:74-94` 同步取号；`:157-166` 确定性测试断言 id 0/1/2 与连接序                                                                                                                 | 该 Gateway 只被 `host-surface`（NDJSON 面）使用；真实 `@escode/rpc` 路径是 `host-rpc/connection.rs` 的每连接循环，不经过 Gateway lane。生产路径上“到达序 = 登记序”无证据，故为 PARTIAL                                                                                                                    |
| H1.4  | PARTIAL | 运行时扇出：`host-runtime/src/instance.rs:29-37,123-145` 满即摘除并置 `lagged`，容量 4096/1024；host-surface：`host-surface/src/lib.rs:30,315` 每连接 256、`try_send` 丢弃可重建流                    | 与 §3.2 的 runtime/host-surface 行为一致；但 §3.2 的 “Gateway 入站 256 背压”在真实 RPC 路径不存在（`connection.rs` 是字节水位队列）；Host 日志 `ControlWriter::log`（`host-control/src/lib.rs:208-223`）队满时任何级别都丢，不是“warn 起不丢”                                                             |
| H1.5  | FAIL    | `host-rpc/src/connection.rs:30-32,44-78,150-155` 实现 8/2/64 MiB 与 hard-limit 关闭；`rg` 在全部 host 测试中找不到 saturated/drained/超上限用例                                                       | 代码存在，但 spec 要求的三类测试缺失                                                                                                                                                                                                                                                                      |
| H1.6  | PARTIAL | `host-control/src/lib.rs:238-307` 单队列顺序、控制等待/日志丢弃；`tests/host_entry.rs:141-229` attach → 请求 → detach → shutdown exit 0                                                               | 进程级测试只断言退出码 0，没有断言 `Stopped{residual:[]}`；残余为空只在 `instances.rs:199-215` 的单测中断言。验收口径要求进程级看到 `residual: []`，故为 PARTIAL                                                                                                                                          |
| H1.7  | PASS    | `tests/instances.rs:169-197` 4 个并发 `get` 只启动一个实例；`:146-166` 关闭后懒重启 generation +1                                                                                                     | 真并发（多个 `tokio::spawn` 同时 get），不是顺序调用                                                                                                                                                                                                                                                      |
| H1.8  | PASS    | `tests/instances.rs:273-289` child 用例在 CLI 二进制在场时执行并通过（本轮输出含 `child_link_serves_the_same_ready_frame ... ok`）；移走二进制后显式打印 `skip: escode-cli-rust 二进制不在场`（§5-5） | 注意 skip 仍被测试框架计为 `ok`，CI 上若未构建 CLI 会“绿但未测”                                                                                                                                                                                                                                           |
| H1.9  | PASS    | `crates` 无进程 env 读取（边界脚本 0 处，负向见 §5-3）；`dataBaseDir` 来自 `rpcListen`（`src/main.rs:183,217`）                                                                                       | `src/main.rs:61,65` 仍有 `ESCODE_HOST_RUNTIME_LINK`/`ESCODE_HOST_RUNTIME_BINARY` 两处裸 `std::env`（组合根，不在 crates 门禁范围）；实例 `home` 由 `src/control.rs:194-197` 读进程 `HOME` 得到，生产上由 Main 在 spawn 时把 `HOME` 设为 `rpcListen.home`（`rustHostBridge.ts:87-90`），语义等价但存在耦合 |
| H1.10 | PASS    | `tests/rpc_surface.rs:27-137`：无/错密钥 401、带 Origin 403（HTTP 与 WS）、两条 WS 路径无 capability 401、token 一次性复用 401；`:237` 非回环 bind 拒绝                                               | 失败路径 fail-closed                                                                                                                                                                                                                                                                                      |

### H2 唯一 Host

| ID   | 结论       | 证据                                                                                                                                                                                                                                                                                                                                           | 备注                                                                       |
| ---- | ---------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------- |
| H2.1 | FAIL       | `rustHostBridge.ts:36-40` 仍有 `ESCODE_RUST_HOST` 开关；`rustHostManager.ts:50-53,99-103` 启动失败“falling back to Node Host”；`index.ts:836-839` 注释明确回退；`index.ts:864-879` 仅在开关打开时注入 `attachRustHostLocal`                                                                                                                    | 运行时路径仍存在 Node Host 分支                                            |
| H2.2 | FAIL       | 无“启动失败 → 明确报错”的测试；失败路径是 manager 记 warn 后 throw，调用方回退 Node Host（`rustHostManager.ts:99-103`）；`webRemoteControlSharedHostAttachments.ts:137-151` 在无 Rust attachment 时直接走 `attachLocalHost`                                                                                                                    | 静默降级而非显式错误                                                       |
| H2.3 | FAIL       | `desktopHostProcess.ts:742-770` 窗口自身 host 仍是 Electron utility process：`MessageChannelMain`、`broadcastHub.register`、`taskRealtimeBus.registerHost`、`registerHostProcess` 均按 utility process 处理；`rustHostManager.ts:16-19` 自认“未接线”                                                                                           | 唯一 Host 目标不成立                                                       |
| H2.4 | UNVERIFIED | `DISPLAY`/`WAYLAND_DISPLAY` 均为空，无图形会话；未启动 App，无法采集进程树                                                                                                                                                                                                                                                                     | 环境受限，未执行                                                           |
| H2.5 | UNVERIFIED | 无 `packages/desktop/dist`，未构建安装包                                                                                                                                                                                                                                                                                                       | 未执行                                                                     |
| H2.6 | FAIL       | 实际清单：46 channels / 539 members / 334 Implemented / 205 Pending；13 个 channel 全 Pending：`bots`、`hooks`、`off-peak-task`、`plugin-sync`、`cloud-content`、`conversation-share`、`cua-permission`、`cua-pip-session`、`marketing-touch`、`plugin-ui-app-tools`、`plugin-ui-bridge`、`plugin-ui-sampling`、`provider-provisioning-target` | Renderer 实际调用的 Pending 见下方“阻塞项”；不能按“缺口显式可见”视为可上线 |
| H2.7 | FAIL       | 只有优雅关闭后的懒重启（`instances.rs:146-166` 调 `close`）；无杀进程/崩溃注入用例；`rg` 在 host 测试中找不到 SIGKILL/crash                                                                                                                                                                                                                    | 唯一 Host 下这是唯一故障隔离手段，缺测试                                   |

H2.6 的 Renderer 实际调用证据（UI/服务定位）：

- `hooks`：`packages/ui/src/store/hooksStore.ts:114,133,199,228` 调 `loadHooks`/`saveHooks`；`grantWorkspaceHookTrust` 经 `packages/services/src/node.ts:1435` 暴露。三个成员全部 Pending。
- `bots`：`BotsDialog.tsx:177,228,247-250,320,340,371,487,764,805,958,964,997` 调 `getConfig`/`getStatus`/`listWorkspaceRefs`/`getBotStates`/`saveBot`/`createBindCode`/轮询注册/`resetBotState`/`removeBotSecret`/`deleteBot`；`useBotGroupTask.ts` 等调 `resendGroupResult`/`reconcileGroupResult`/`retryTopicPreparation`；`Root.tsx:386` 调 `syncAppRuntimePreferences`。32 个成员全部 Pending。
- `off-peak-task`：`packages/ui/src/store/offPeakTaskStore.ts:179`、`hooks/useOffPeakTaskNotifications.ts:37` 调 `list()`；11 个成员全部 Pending。
- `plugin-sync`：`packages/ui/src/settings/RemotePluginSyncDialog.tsx:789,798,1700` 调 `exportPluginsArchive`/`importPluginsArchive`/`checkRemoteUserPluginWriteAccess`；7 个成员全部 Pending。
- `client-config.getOffPeakClientConfig`：`offPeakTaskStore.ts:175` 调用，Pending。
- `media-preview.refreshPlaybackUrl`/`release`：`packages/ui/src/PreviewPane.tsx:1082-1083` 调用，Pending。
- `escode-agent`：`hooks/useSessionSubagents.ts:64` 调 `listSessionSubagents`；`hooks/useSessionDebug.ts:33` 调 `readSessionDebug`；两者 Pending。`App.tsx:831` 的 `listPlugins` 另经 `plugin-management.listPlugins` 已实现，不能仅按同名判定。
- `escode-session`：`getWorkspaceRuntimeIdentity`、`readSessionEvents`、`respondProviderRuntimeHeaders` Pending；前两者在服务接口中存在（`escodeSession.ts:156`），是否被当前 UI 调用未找到直接证据。
- `gen-ui.prepareSandbox`、`provider-provisioning-target.apply` Pending；`cua-*` 与 `plugin-ui-*` 属范围边界登记的不迁/阻塞项（`rust-u2-host-scope-boundary.md:8-30`）。

### H3 存量数据单向读入

| ID   | 结论    | 证据                                                                                                                                                                                                                                                         | 备注                                                                                                                                                                                                                                                                            |
| ---- | ------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| H3.1 | PASS    | `tests/escode-settings-host-interop.test.ts` 本轮 1 passed（真实 TS `settingService` ↔ 真实 Host 进程，3 轮交替、未知键保留、缩进）；`settings.rs:129-208` 单测覆盖 TS 形状读入与顶层合并                                                                    | 注意：根键用 `.sort()` 后比较（L3），值逐项比较（L2），不是整对象 L4                                                                                                                                                                                                            |
| H3.2 | PARTIAL | `settings.rs:21-22,38-58` 实现 3×300ms 重试、坏文件 `.corrupt-*` 隔离、合法非对象不隔离；`:163-189` 只测隔离与不隔离，没有半截 JSON 在重试后恢复的用例                                                                                                       | 与 TS 逐分支对照未完成；默认值语义见 H4 settings                                                                                                                                                                                                                                |
| H3.3 | PARTIAL | DDL/账本：`tasks_index/schema.rs`（生成资产）、`mod.rs:352-365` 自校验、`ledger.rs` 三分类；生成器 `--check` exit 0；`write_state.rs:112-130` `BEGIN IMMEDIATE`；`rustHostRpc.test.ts:213-411` 用真实 `TaskIndexRepo` 写库并 `toEqual`                       | `busy_timeout` 是 **25ms**（`tasks_index/mod.rs:114-116`），不是任务口径的 5000ms；TS 长连接 Repo 用 5000（`taskIndexRepo.ts:543-545,588`），Rust 同连接承担 Host 读写且无 SQLITE_BUSY 重试                                                                                     |
| H3.4 | PARTIAL | 已确认可读：settings、tasks-index、插件布局、skills、commands、subagents、credentials、provider 个人配置、onboarding、telemetry deviceMid、checkpoints、gen-ui、memory、model trajectory、Claude 导入（`crates/host-services/src/*`）；会话库由 runtime 转发 | 已确认缺口：hooks 信任锁（`workspace-hook-trust-v1.json` 在 `apps/escode-host` 无读取者）、bots 配置/状态/附件、`cli/workflows` JSON、`tmp/paste-attachments`、`v2/sessions/*.json` 旧快照读取、`v2/session-bindings`；`v2/session-bindings` 仅存储分类出现。逐项读入测试未建立 |
| H3.5 | FAIL    | `src/embed.rs:11-12,115-146` 明确“不做 TS 历史导入”，`run_embedded` 直接 `Store::open` Rust 库；`apps/escode-host` 无 `import_history`/`legacy_import` 调用                                                                                                  | Rust CLI 的导入有幂等账本与测试（`legacy_storage.rs:25-36,60-77`；migration-scale/lifecycle 测试），但默认 embedded Host 不会触发，Host 首启接管不成立                                                                                                                          |

### H4 分域验收

| 域              | TS oracle（现场核对）                                                                      | 对拍级别                                                                                                             | 真实客户端/差分                                                         | 已登记差异                                                                       | 结论                                                                                                                                |
| --------------- | ------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------- | -------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| settings        | `packages/services/src/setting/settingService.ts:63-65,116-180,276-324`                    | 单测 L4；interop L3（根键排序）+ 逐值 L2；真实客户端 L2                                                              | `rustHostRpc.test.ts:28-59`；interop 1/1                                | 无正式差异表；`rust-u2-user-config-services.md:73` 只有原因无移除条件            | PARTIAL：读写主路径可用，但缺文件/坏文件回退 `{}` 而 TS 返回 `defaultSettings()`（schema 默认键），且无 zod 校验/legacy 迁移/写队列 |
| system          | `systemService.ts:352-364`                                                                 | `rpc_session.rs:274-302` L4；`rustHostUserConfig.test.ts:390-487` 对 probeIntranet 的 latency/错误文案做归一化（L2） | 真实客户端 `rustHostRpc.test.ts:435-440`                                | `rust-u2-user-config-services.md:86-88` 有原因无移除条件                         | PARTIAL                                                                                                                             |
| file            | `fileService.ts:40-42,86-103,176-195,371-424,477-518`                                      | `readdir` 与真实 TS `toEqual` L4（但只跑一种进程 locale；语料 24 项）；其它 L2/L4                                    | `rustHostRpc.test.ts:90-211`；`domains_file_tests.rs`                   | errno 文案差异有原因无移除条件（`handoff:191`）；file index 差异有原因无移除条件 | PARTIAL：spec 声称的四 locale 差分不存在（`collation_tests.rs:50-118` 仅 en-US）                                                    |
| escode-session  | `escodeSessionService.ts:217-487`                                                          | `rpc_session*.rs` L2/L4；真实客户端只覆盖 list/read/close 与 create 错误路径                                         | `rustHostRpc.test.ts:70-85,443-481`                                     | 账号 provider 同步差异有原因无移除条件                                           | PARTIAL：create/resume 成功路径没有真实客户端或 TS 差分                                                                             |
| tasks-index     | `taskIndexRepo.ts:1505,1636,1835,1930,2032,2165,2628`                                      | 真实 TS Repo `toEqual` L4；DDL 资产 L5（sha256）                                                                     | `rustHostRpc.test.ts:213-411`；`task_projection/fixture_tests.rs`       | meta_json 浅校验、搜索大小写折叠有原因；多数无移除条件                           | PARTIAL：busy_timeout/重试差异未登记                                                                                                |
| escode-agent V4 | `escodeAgentConnectionScope.ts:91,159-187,237,705`；`escodeAgentService.ts:5882,5987,6424` | `scope_routing.rs:159-223` L4 有序；`rpc_agent_v4.rs:285-345` L2/L3（`contains(REPLY)`、跳过其它帧）                 | `rustHostAgentV4.test.ts:22-86` 只覆盖 lifecycle 与 sessions-index 事件 | `rust-u2-agent-v4.md:123-142` 14 条，仅第 1/2 条有移除条件                       | PARTIAL：ACK 先于首帧、关闭即注销没有 E2E；真实客户端未覆盖 conversation 命令/帧                                                    |

抽查（读代码核对）：

- `readdir`：`domains_file_readdir.rs` + `collation.rs` 按 `LC_ALL > LC_MESSAGES > LANG > en-US` 解析并用 ICU4X `CollatorBorrowed`，实现与 spec 描述一致；但四 locale 语料证据缺失。
- `checkFilesExist`：`domains_file.rs:23,83-122,321-360` 批上限 15、`isFile()`、60s/100 LRU；缓存测试只断言长度，不验证淘汰最旧项或命中刷新（`domains_file_tests.rs:8-23`）。
- `readTextFile`：`domains_file.rs:140-222` 长度 clamp、NUL 立即二进制、控制字符 >0.3、`content` 空串；测试覆盖边界。
- settings 重试：`settings.rs:21-22,38-58` 参数正确；测试没有覆盖“撞上半截 JSON 后第 2/3 次成功”。
- tasks-index：DDL 字节冻结 + sha256 双校验 + `BEGIN IMMEDIATE` 均在；`busy_timeout` 与任务口径不符。
- V4 投递：`agent_v4/scope.rs` 的 ACK 前暂存、ACK 后按到达序释放、只释放匹配 subscriptionId 的帧、1024 帧/32MiB 溢出显式失败均存在；缺端到端守护。

spec 引用行号漂移：`rust-u2-tasks-index.md` §6.1 的 TS 行号整体偏 8–18 行（例如 `resumeSnapshot` 写 1175、实际 `escodeTaskServiceAdapter.ts:1183`；`snapshotToMeta` 写 1408、实际 1416；`limitTaskSnapshotMessages` 写 3664、实际 3681）。

### H5 链路语义

| ID   | 结论    | 证据                                                                                                                                                                                                                                                | 备注                                                                                                                                                                                                                                        |
| ---- | ------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| H5.1 | FAIL    | `rg` 在 `packages/desktop/test/e2e` 只找到 `desktop-continuous` 的证据字段/注释，没有 `web-remote-replayable` 的 E2E；双语义只在单测/集成测试出现（`escodeAgentService.v4.test.ts:482,634` 等）                                                     | AGENTS.md 要求两种链路都验证；当前没有                                                                                                                                                                                                      |
| H5.2 | PARTIAL | Node 路径复用测试：`webRemoteControlSharedHostAttachments.test.ts:29,102`、`desktopRemoteSessions.test.ts:641,796`；Rust dev 路径只注入本地 workspace 的 attachment（`index.ts:864-879`），远程 workspace 仍走 `desktopRemoteSessions` 的 Node Host | “手机远控复用 Rust Host 已有 attachment”只在本地 workspace 成立                                                                                                                                                                             |
| H5.3 | FAIL    | 多窗口共享实例：`host_entry.rs:196-207` 第二窗口 generation 仍为 1；`ws_surface.rs:174-211` 同实例扇出                                                                                                                                              | 无窗口调度未迁移：窗口关闭销毁 Host（`desktopWindowLifecycle.ts:266-273`），`desktopCronScheduler.ts:119-128,157-166` 无 Host 时回 transient；Rust 控制协议无 `CronRun`/`OffPeakRun`（`host-control/src/lib.rs:37-116`），automation 域未迁 |

## 5. 负向验证记录

所有改动均在独立工作树 `/tmp/escode-acc-wt`（detached `89eee7c4`）中进行，完成后已 `git worktree remove --force`；主工作区 `git status --short` 为空。另有 `/tmp/escode-head-check` 属其他工作流，未触碰。

1. **R1 环境读取门禁**：在 `crates/model/src/lib.rs` 非测试代码追加 `std::env::var("ACCEPTANCE_PROBE")`。
   - 预期：边界脚本失败。
   - 实际：`model/src/lib.rs: reads process env outside core-api/src/env.rs`，exit 1。
   - 恢复后脚本恢复 OK。

2. **R5 spawn 纪律**：在同一文件追加 `fn ... { tokio::spawn(async {}); }`。
   - 预期：若有机器守护，应失败。
   - 实际：`Rust workspace boundaries: OK`，exit 0。**结论：spawn 纪律无机器守护**；现有直接 `spawn_blocking` 也不会被任何脚本或测试发现。

3. **H1.9 Host 环境门禁**：在 `crates/host-core/src/lib.rs` 追加 `std::env::var`。
   - 预期：Host 边界脚本失败。
   - 实际：`host-core/src/lib.rs: reads process env (Host env arrives via control/RuntimeOptions)`，exit 1（同时列出既存的两个 400 行失败）。
   - 恢复后仅剩既存行数失败。

4. **H1.1 契约清单双向负向**：
   - (a) 在 `IMPLEMENTED` 加入 `("hooks","loadHooks")`（Pending → Implemented）：`source_method_table_matches_implemented_entries` 与 `registered_channels_are_exactly_those_with_implemented_members` 失败，输出 `[("hooks", "loadHooks")]`，exit 101。
   - (b) 删除 `("file","stat")` 登记：`source_method_table_matches_implemented_entries` 与 `pending_entries_are_not_implemented` 失败，输出 `[("file", "stat")]`，exit 101。
   - 两次均恢复，工作树 `git status --short` 为空。

5. **H1.8 child 链路 skip**：把 `apps/escode-cli-rust/target/debug/escode-cli-rust` 移到 `/tmp` 后运行 `child_link_serves_the_same_ready_frame`。
   - 实际：`skip: escode-cli-rust 二进制不在场（先构建 CLI workspace）`，测试仍报告 `ok`。
   - 结论：是显式 skip，不是静默通过；但 CI 上若未构建 CLI，该用例会计为通过而未覆盖 child 链路。

6. **H0 干净 HEAD 复核**：在干净工作树运行 `check-escode-host-boundaries.mjs`，仍失败于 `domains_task.rs`（401）与 `rpc_task_remaining.rs`（405）。说明 H0 不是本报告工作区污染造成的。

## 6. 文档漂移清单

| 位置                                                   | 实际情况                                                                                                                                                                                                     | 建议                                                                                                                                                                                                                       |
| ------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `docs/specs/rust-u1-runtime-lib.md` §2 第 4 条 vs §3.4 | §2 说 `RuntimeOptions` 含 `config`；§3.4 结构体没有；`src/embed.rs:31-40` 实际有 `config: Option<PathBuf>`                                                                                                   | 以 `embed.rs` 为准补 §3.4 的 `config` 字段与说明（§2 是对的）                                                                                                                                                              |
| `docs/specs/rust-u1-runtime-lib.md:80`                 | 写“迁移点（u1 收口）：……tools 20 处……”，读作已全部改经 Spawner；实际 tools 仍有 14 处裸 `tokio::task::spawn_blocking`，另有 `state/src/storage.rs:131`、`wiki/src/pipeline.rs:156`                           | 改为“已迁 N 处、剩余清单见 R5”，或完成迁移后再保留原句                                                                                                                                                                     |
| `docs/specs/rust-unified-host.md` §7 第 5 条           | 写“投递层测试/进程树三补丁/unwrap 热点——待做”，同文件阶段 0 已全部 `[x]`                                                                                                                                     | 改为已完成，或删除与阶段 0 重复的句子；unwrap 计数若要保留，注明快照日期                                                                                                                                                   |
| `docs/specs/rust-unified-host.md` §7 第 3 条           | 写“deny.toml 待 cargo-deny 接入”，阶段 0 已写 cargo-deny 接入 CI                                                                                                                                             | 改为“已接入”；本机缺 `cargo-deny` 只是环境限制                                                                                                                                                                             |
| `docs/specs/rust-u2-host-skeleton.md`                  | 两个 `## 8`（第 156 行验收、第 169 行风险）                                                                                                                                                                  | 风险改为 `## 9`                                                                                                                                                                                                            |
| `docs/specs/rust-u2-host-skeleton.md:59,137`           | runtime 扇出“满即摘除并置 lagged”与 host-surface“try_send 丢弃可重建流”是两层不同队列，代码分别对应 `instance.rs:123-145` 与 `host-surface/src/lib.rs:315`；第 59 行括号里的“本表原文「满时阻塞」”是历史说明 | 保留两层描述但标明层级；删除或改写历史括号，避免读者以为二者冲突。另：§3.2 的 Gateway 入站 256 背压在真实 RPC 路径不存在，应改为“host-rpc 字节水位队列”；Host 日志应改为“队满时任何级别可丢并计数”（代码不是 warn 起不丢） |
| 交接手册 `rust-unified-host-handoff-2026-10-15.md:8`   | “51 channel / 619 成员”                                                                                                                                                                                      | 实际为 **46 channels / 539 members / 334 Implemented / 205 Pending**（2026-10-18 本轮生成器与脚本统计）                                                                                                                    |
| 交接手册 `:30`                                         | “101 passed”                                                                                                                                                                                                 | 本轮 `cargo test --workspace` 为 **721 passed / 0 failed**（54 个测试二进制）；“101”已过期                                                                                                                                 |
| 交接手册 `:213`                                        | 说 `escode-task.createTask` 未实现                                                                                                                                                                           | 当前 `escode-task` 已 76/76 全量收口（Pending 列表中无该 channel）；真实客户端旧断言 `rustHostRpc.test.ts:416` 也已过时                                                                                                    |
| `docs/specs/rust-u2-host-skeleton.md:130`              | 说 `searchWorkspaceFiles`/`listWorkspaceFilesRange`/`workspaceFileListLength` 未实现、readdir 是缺口                                                                                                         | 当前 file 已 17/17（`inventory_services.rs` 无 file 的 Pending 成员）；应改为已交付或注明日期                                                                                                                              |
| `docs/specs/rust-u2-host-skeleton.md:131`              | 声称 28 项语料在四种 locale 下各跑一遍                                                                                                                                                                       | 实际 `rustHostRpc.test.ts:166-208` 只跑测试进程当前 locale，语料 24 项；`collation_tests.rs:50-118` 只有 en-US。应改为“实现按 locale 解析，证据为单 locale；四 locale 矩阵待补”                                            |
| `docs/specs/rust-u2-host-skeleton.md:127`              | 说坏文件“回退默认值（TS settingService 同语义）”                                                                                                                                                             | Rust 返回 `{}`，TS 返回 `appSettingsSchema.parse({})` 的完整默认对象（`settingService.ts:63-65`）；应登记为差异或补齐默认值                                                                                                |
| `docs/specs/rust-u2-tasks-index.md` §6.1               | TS 行号偏 8–18 行                                                                                                                                                                                            | 用当前 `escodeTaskServiceAdapter.ts` 的函数位置重写引用，或改为函数名引用                                                                                                                                                  |
| `docs/specs/rust-u2-host-skeleton.md:138`              | 写进程级端到端断言 `Stopped{residual:[]}`                                                                                                                                                                    | `tests/host_entry.rs:83-95` 只断言退出码 0，未读 `Stopped` 帧；应补断言或改文                                                                                                                                              |
| `docs/specs/rust-u2-host-skeleton.md` §3.1             | Host 每域持一个 Spawner、`running()` 为空                                                                                                                                                                    | Host crates 只在 `host-runtime/src/link.rs:53` 使用 `Spawner` 承载 runtime；Host 自身约 60 处 `tokio::spawn`，没有 per-domain Spawner/`running()` 断言。应改文或补实现                                                     |
| `AGENTS.md:61`                                         | “每个窗口使用一个 window-scoped Local Host”                                                                                                                                                                  | 与目标“一个 App 一个 Host”冲突；改为“每个 App 一个 Rust Host；窗口经 attachment 接入同一 Host；本地 workspace 共享实例，远程 workspace 由 attachment 注册表路由”                                                           |

### 双 Host 前提段落及建议措辞

以下段落都以“Node Host 与 Rust Host 可互切/双向写读/失败回退”为前提，与单 Host 目标冲突：

| 位置                                   | 现文要点                                                                | 建议                                                                                                                                     |
| -------------------------------------- | ----------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| `rust-unified-host.md:66-70`           | “双运行期数据互操作……必须双向可读，否则 dev 开关来回切会破坏状态”       | 改为“迁移期一次性单向读入：Rust Host 只读 Node Host 存量；settings/tasks-index 同文件同格式仅用于过渡；Node Host 删除后不再承诺反向可读” |
| `rust-unified-host.md:149`             | settings“双 Host 交替写读演练通过”                                      | 改为“以 TS 写出的冻结夹具验证 Rust 单向读入；交替写读仅作灰度期回归，不作验收门槛”                                                       |
| `rust-unified-host.md:183`             | “child 模式保留（差分、回退、故障隔离选项）”                            | 改为“child 仅用于差分/诊断与故障隔离实验，不是运行时回退路径”                                                                            |
| `rust-unified-host.md:187`             | “Rust Host 在 dev 开关下跑通全部桌面 E2E（embedded 与 child 双链路）”   | 改为“embedded 为主链路跑全部桌面 E2E；child 只跑同一场景的回归子集”                                                                      |
| `rust-unified-host.md:189`             | “双 Host 交替写读演练”                                                  | 删除该验收，换成“Node 写 → Rust 单向读入”的夹具验收                                                                                      |
| `rust-unified-host.md:197`             | “四平台 Node→Rust 切换真机演练 + 回退演练”                              | 改为“四平台切换演练 + 安装包版本回滚演练；不保留 Node Host 运行时回退”                                                                   |
| `rust-unified-host.md:312`             | L2“持久化跨读（Rust 读 Node 写的库、反向亦然）”                         | 改为“Rust 读 Node 写的库（单向）；反向读仅在迁移期作为诊断，不作验收”                                                                    |
| `rust-unified-host.md:313`             | L3“桌面 E2E 双 Host 跑同一场景集”                                       | 改为“Rust Host 跑场景集；Node Host 仅作冻结前的 oracle，不参与运行时矩阵”                                                                |
| `rust-unified-host.md:381-382`         | 风险对策“dev 开关 + 与 TS Host 全量差分”“child 模式作故障隔离/回退”     | 改为“迁移期与 TS oracle 差分；切换后 goldens + spec”“child 仅诊断/隔离，不回退”                                                          |
| `rust-u2-host-skeleton.md:13`          | “dev 开关与回退链”                                                      | 改为“切换期开关与显式失败”                                                                                                               |
| `rust-u2-host-skeleton.md:100,102`     | 分域验收写“Node/Rust 双 Host 交替写读”“Node Host vs Rust Host 双跑差分” | 改为“TS 夹具单向读入”与“Rust Host 对冻结 goldens”                                                                                        |
| `rust-u2-host-skeleton.md:110-114`     | §6“dev 开关与回退”                                                      | 改为“切换期开关：默认 Rust Host；启动失败显式报错；child 仅诊断，不自动回退”                                                             |
| `rust-u2-host-skeleton.md:148,150,175` | CI interop、失败回退、双 Host 状态互操作                                | 相应改为单向读入夹具、显式失败、过渡期格式兼容                                                                                           |
| `AGENTS.md:61`                         | window-scoped Local Host                                                | 见上表建议                                                                                                                               |

## 7. 上线阻塞项

### 阻塞“切到唯一 Rust Host”

1. **H0 门禁红**：`domains_task.rs` 401 行、`rpc_task_remaining.rs` 405 行；`pnpm fmt:check` 红在 `bots-v4-stream.md`。门禁不绿不能切默认。
2. **H2.1/H2.2/H2.3**：`ESCODE_RUST_HOST` 开关和 Node Host 回退仍在；启动失败静默降级；窗口自身 host 仍是 Electron utility process。唯一 Host 目标不成立。
3. **H2.6 Renderer 可见缺口**：`hooks`（3/3 Pending）、`bots`（32/32）、`off-peak-task`（11/11）、`plugin-sync`（7/7）、`client-config.getOffPeakClientConfig`、`media-preview.refreshPlaybackUrl`/`release`、`escode-agent.listSessionSubagents`/`readSessionDebug`、`gen-ui.prepareSandbox`、`provider-provisioning-target.apply`。这些在 dev 开关下会直接 `NotImplemented`。
4. **H2.7**：没有杀实例的崩溃隔离测试；唯一 Host 下缺故障隔离证据。
5. **H3.5/H3.4**：Host 默认 embedded 不做 TS 会话库导入；hooks 信任锁、bots 状态、workflow JSON、paste attachments 等没有读入实现或测试。
6. **H5.1/H5.3**：`web-remote-replayable` 没有 E2E；无窗口 cron/offpeak 未迁移（automation 域），窗口全关后调度不可用。
7. **settings 默认值语义**：缺少文件/坏文件时 Rust 返回 `{}`，TS 返回完整默认对象。切到唯一 Host 后设置消费者会看到不同视图；需补齐默认值（或明确由 Renderer 合并默认值，并登记）。

### 不阻塞切换（但必须在验收前处理/登记）

1. R5：14 处 tools 直接 `spawn_blocking`，绕过 Spawner；影响 `running()` 残余清单与 panic 上报。
2. R3：补“两个 workspace 各自运行工具，文件落在各自目录”的测试。
3. R6：双传输补 `MAX_REQUEST_BYTES` 超限用例。
4. H1.5：补 saturated/drained/64 MiB 关连接的测试。
5. H1.4：修正 Gateway 入站 256 与日志级别策略的文档/实现口径。
6. H1.6：进程级补 `Stopped{residual:[]}` 断言。H1.3：在真实 `host-rpc` 路径上补到达序测试，或在 spec 中说明该路径的序语义由每连接循环保证。
7. H4 settings/file/heartbeat 等：补未登记差异的 reason + 移除条件；补四 locale readdir 证据；修正 `busy_timeout` 或登记。
8. R9：修两个既存 TS 类型错误，恢复完整套件入口；本机补 `cargo-deny` 或由 CI 证明。
9. 性能 harness：修 Linux CPU 时间解析（`Invalid CPU time: 2-12:46:17`），再谈 A/B 数字。
10. 陈旧测试：`rustHostRpc.test.ts:62` 的 serial 断言按当前实现更新。
11. 文档：见 §6 全部漂移项。

## 8. TS oracle 依赖清单（golden freeze）

### 当前依赖

| 类别                       | 数量/位置                                                                                          | 说明                                                                                                                                                                    |
| -------------------------- | -------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| CLI 实时 Node oracle       | `packages/services/tests/escode-cli-rust-*.test.ts` 共 165 个文件                                  | 多数以 `apps/escode-cli/packages/cli/dist/escode.cjs` 与 Rust 二进制双跑；`scripts/test-escode-cli-rust.mjs` 驱动                                                       |
| 直接 import TS 源码 oracle | 迁移/工具/凭据/提示词等测试（如 `escode-cli-rust-migration*.test.ts`、`tool-parity`、`questions`） | 依赖 TS 会话库、schema、handler                                                                                                                                         |
| Desktop TS Host oracle     | `packages/desktop/test/rustHost*.test.ts` 25 个文件                                                | `rustHostRpc.test.ts` 用真实 `createFileService` 与 `TaskIndexRepo` 做差分；`rustHostAgentV4.test.ts` 等用真实 `@escode/client`                                         |
| 双 Host interop            | `scripts/test-escode-host-interop.mjs` → `escode-settings-host-interop.test.ts`                    | 真实 TS 设置服务 ↔ 真实 Host 进程，3 轮交替写读；单 Host 目标下应改为单向读入                                                                                           |
| parity 差分                | `scripts/diff-escode-cli-rust-parity.mjs`                                                          | 实时双跑；`approved.json`/`baseline.json` 只是差异登记/指纹，不是完整 Node golden；evidence 输出 `.escode-runtime/parity` 被 `.gitignore:83` 忽略；脚本没有 golden 模式 |
| 生成器                     | 31 个 CLI 生成器 + 8 个 Host 生成器 `--check`                                                      | 多数直接 import TS 源码；删除 TS 后 `--check` 失去 oracle                                                                                                               |

### 已自包含的 golden

- Host：`task_snapshot.golden.json`、`snapshot_golden.json`、`commit_message_golden.json`、`frontmatter_golden.json`、`policy_golden.json`、`plugin_schema_golden.json`、`controller_frames.golden.json`（`find apps/escode-host -name '*golden*.json'`）。
- CLI：`crates/domain/tests/fixtures/`、`crates/tools/tests/fixtures/` 等 26 个 JSON（grep 计数），通过 `include_str!` 消费。
- 这些资产删除 TS 后仍可加载；但“资产等于 TS 行为”的生成链仍依赖 TS 源码。

### 结论

**TS Host 代码暂不可删除。** 需要：

1. 为 165 个 CLI 测试与 25 个 desktop Host 测试录制 Node/TS 侧 observation 为仓库内 golden（含生成器与 `--check`）。
2. 给 `diff-escode-cli-rust-parity.mjs` 增加 golden 模式（不再实时跑 Node），并把 evidence 落仓。
3. 把 interop 改为“TS 写出的冻结 setting.json → Rust 单向读入”的夹具。
4. 把 `rustHostRpc.test.ts` 的 TS `createFileService`/`TaskIndexRepo` 差分改为冻结输出；`rustHostAgentV4` 保留真实 `@escode/client`（它不是 oracle，是客户端契约）。
5. 上述清零后，才可删除 TS Host/TS CLI 并切换 `ESCODE_RUST_HOST` 为默认。

## 9. 跨仓对照（ZCode-rs）

参考仓库已 `git clone --depth 1` 到 `/tmp/ZCode-rs` 只读查看（未修改）。

### 9.1 验收项映射

ZCode-rs `docs/specs/rust-u1-runtime-library.md` §5：

| ZCode-rs 验收项                                                    | ESCode 证据                                                                                          | 结论    |
| ------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------- | ------- |
| 现有测试全部通过（`check:rust`、`test:zcode-cli-rust`）            | `check:escode-cli-rust` 通过；`test:escode-cli-rust` 被既存 TS 类型错误挡住；Rust workspace 455 通过 | PARTIAL |
| 进程 cwd/env 与 runtime 不同，runtime 用 `RuntimeOptions`          | `embed_memory_link.rs:228-263`                                                                       | PASS    |
| 同一 runtime 两个 workspace 各自创建会话并运行工具，文件落各自目录 | `embed_memory_link.rs:163-200` 只验证会话隔离，未运行工具/文件                                       | MISSING |
| 关闭写端后 task 完成，同进程再 spawn 成功                          | `embed_memory_link.rs:204-226`                                                                       | PASS    |
| 非测试代码 `std::env::var*`/`current_dir` 为 0                     | 边界脚本 + 独立 rg；测试外仅 `core-api/src/env.rs`                                                   | PASS    |

ZCode-rs `docs/specs/rust-host.md` §7：

| 检查                                                               | ESCode 证据                                                                   | 结论    |
| ------------------------------------------------------------------ | ----------------------------------------------------------------------------- | ------- |
| TS：typecheck/lint/fmt/architecture                                | 前三通过；fmt 红在 `bots-v4-stream.md`                                        | PARTIAL |
| Rust：fmt/clippy/test                                              | 三者单独通过；H0 的 400 行边界失败                                            | PARTIAL |
| 契约覆盖率、wire 夹具、差分测试                                    | 契约 334/539；wire 帧头夹具在 `rpc_surface.rs`；parity 仍 `continue-on-error` | PARTIAL |
| 每个里程碑同时验证 `desktop-continuous` 与 `web-remote-replayable` | 无 replayable E2E                                                             | FAIL    |

### 9.2 保留清单

| 保留项                   | 代码位置                                                                                       | 结论 |
| ------------------------ | ---------------------------------------------------------------------------------------------- | ---- |
| owner 锁 + exit 75       | `crates/state/src/storage.rs:59,127-173`；`src/main.rs:59`                                     | 保留 |
| 跨进程 `file_lock.rs`    | `crates/host/src/file_lock.rs`                                                                 | 保留 |
| Windows 代码页解码       | `crates/host/src/output_encoding.rs`                                                           | 保留 |
| `credential_cipher`      | `crates/host/src/credential_cipher.rs`                                                         | 保留 |
| OAuth                    | `crates/tools/src/mcp_oauth_flow.rs`、`mcp_oauth_lead.rs`                                      | 保留 |
| MCP inputSchema 键序捕获 | `crates/tools/src/mcp_http_capture.rs`、`mcp_sse.rs`、`mcp_connection.rs`（`mcp_raw_capture`） | 保留 |
| model-IO JSONL           | `crates/model/src/model_io.rs`                                                                 | 保留 |
| EOF 停滞看门狗           | `crates/app-server/src/stdio_input.rs:19-149`                                                  | 保留 |
| 单管道输出保序           | `crates/app-server/src/stdio.rs:37-65` 单 writer 线程                                          | 保留 |
| embedded-search prelude  | `crates/tools/src/embedded_search.rs` + `tests/fixtures/embedded_search_prelude.json`          | 保留 |
| Read 重复读去重桩        | `crates/tools/src/file_state.rs:1`                                                             | 保留 |
| 子代理权限模式           | `crates/core/src/app/subagents.rs:135-151`、`crates/domain/src/agent_profile.rs:62-63`         | 保留 |
| cron/offpeak 语义        | `crates/core/src/app/cron_tool.rs`、`off_peak.rs` + corpora                                    | 保留 |

### 9.3 Host 进度对照（复核补充）

依据 ZCode-rs `docs/specs/rust-host.md` §4.4、§5.1（克隆 commit `909c6554`，2026-10-11）：

| 维度      | ZCode-rs                                                                                                                 | ESCode `dev`                                                                                  |
| --------- | ------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------- |
| 唯一 Host | 未达成：`ZCODE_DESKTOP_HOST=rust` 为 dev 开关，默认仍 Node Host；切换里程碑 R7“未开始”                                   | 未达成：`ESCODE_RUST_HOST` 开关 + 启动失败回退 Node Host（H2.1）                              |
| 域覆盖    | R1–R6 完成，含 automation、闲时任务、分享、bots（飞书/Telegram/Webhook/微信）、远端 workspace；491 成员中仅少量有意 stub | 334/539 Implemented；`bots`/`hooks`/`off-peak-task`/`plugin-sync` 等 13 个 channel 全 Pending |
| 韧性      | D2 完成：自动重启、启动重试与退出、流控（`rust-host-resilience.md`）                                                     | 无崩溃注入测试（H2.7）；H1.5 水位无测试                                                       |

结论：“唯一 Rust Host”标准在 ZCode-rs、ESCode、上游 `zai-org/ZCode`（TS Host）三处均未达成；ESCode 相对参考实现的主要差距在 bots/automation/offpeak/hooks 域与 Host 韧性，可直接以 `rust-host-bots.md`、`rust-host-automation.md`、`rust-host-resilience.md` 对标补齐（参考，不作 oracle）。

### 9.4 许可与来源

- 根 `NOTICE.md:7-9` 声明 ESCode 为 ZCode 衍生作品、Apache-2.0、上游 `NOTICE.md` 逐字保留在 `third-party/upstream-zcode/NOTICE.md`；`apps/escode-cli-rust/Cargo.toml:23` 与 `apps/escode-host/Cargo.toml:24` 声明 `Apache-2.0`。
- 5 个 Rust 文件在注释中明确写“对标 ZCode-rs”（`scope.rs`、`secret.rs`、`waiters.rs`、`file_stamp.rs`、`embed.rs`）；没有逐文件 Apache 头（本仓库惯例是工作区级声明）。
- 结论：**PARTIAL**——工作区级许可与上游 NOTICE 完整，来源说明只覆盖部分移植文件；若要求“每个移植文件保留来源说明”，需补。

## 10. 性能（只记录，不作为通过条件）

- 平台：Linux x86_64（VMware 虚拟机），无图形会话（`DISPLAY`/`WAYLAND_DISPLAY` 为空）。
- 构建类型：debug（本轮只构建了 debug CLI/Host；未构建 release/安装包）。
- 轮数：0。`node scripts/bench-escode-app.mjs --self-test` 在采样阶段失败，未进入真实 A/B。
- 失败：`Invalid CPU time: 2-12:46:17`，位置 `scripts/bench-escode-app-sampling.mjs:26` 的 `parseCpuTime`。这是 Linux `ps` 输出格式（`2-12:46:17` 是 etime 形态）被当作 cputime 解析的缺陷。
- 配置 C（Rust Host + 内嵌 runtime）仍是占位：`scripts/bench-escode-app.mjs:76-80` 的 `unavailable: "Rust Host 尚未落地（阶段 2）..."`。
- 因此本轮没有“当前出厂形态 vs Rust Host + 内嵌 runtime”的可比数字；不引用 2026-10-14 的旧基线作为本轮结果。

## 11. 既存失败与判定依据

1. **`tsc -p packages/services/tests/tsconfig.escode-cli-rust.json` 两个类型错误**（阻塞 `pnpm test:escode-cli-rust`）：
   - `packages/services/tests/escode-cli-rust-sse.ts(3,15): TS2459`：`Message` 在 `escode-cli-rust-fixture.ts:48` 声明为本地 `type`，未导出。
   - `packages/services/tests/escode-cli-rust-topic-import.test.ts(85,49): TS2339`：`m.info.source` 不在 `session/messages` 的消息联合类型上。
   - 判定依据：两份文件在审计基线 `2a18ec7f` 与当前内容逐字相同（`diff` 无输出）；`2a18ec7f..c15a22aa` 未改 `packages/shared/src` 的消息类型；因此不是本轮 bots 提交引入，属既存失败。`pnpm typecheck` 不含该 tests tsconfig，所以 H0 的 typecheck 绿而套件入口红。
2. **`pnpm fmt:check` 红在 `docs/specs/bots-v4-stream.md`**：该文件在审计开始时是未跟踪 WIP，审计期间由并发提交 `c15a22aa` 引入；当前 HEAD 仍红。与 Rust 迁移代码无关，但属于 H0 门禁。
3. **`check:escode-host` 400 行边界**：`rpc_task_remaining.rs` 405 行在本轮开始前已提交；`domains_task.rs` 401 行由 `89eee7c4` 的 bots 改动（新增 4 行）越过 400。两者都在当前 HEAD 复现；干净 worktree 复核同样失败，排除工作区污染。
4. **`rustHostRpc.test.ts:62` 的 serial 断言**：期望 `serial` 未实现，但 `serial` 已 14/14 迁移（inventory 无 serial Pending）。真实客户端 3 文件 7 passed / 1 failed；失败是测试过时，不是产品缺口。
5. **`bench-escode-app --self-test`**：`Invalid CPU time: 2-12:46:17`，是 harness 的 Linux 解析缺陷，不是性能结论。
6. **`cargo deny` 未执行**：本机 `no such command: deny`，`pnpm check:escode-cli-rust:deny` exit 101；CI 用 `EmbarkStudios/cargo-deny-action` 覆盖，但本机 UNVERIFIED。
7. **freshness 脚本**：`git fetch` 网络失败（TLS），exit 1；未影响后续只读检查。
8. **CI 口径**：`rust-runtime-ci.yml` 的 push 触发不含 `dev`（只 `main`/`feat/**`）；checks 作业只 ubuntu；parity 步骤 `continue-on-error: true`。因此“CI 三平台全绿”不能作为当前门禁已过的证据；test 作业确实三平台跑 `test:escode-host`。

## 12. 结论摘要

R 线的库化本体（显式环境、多实例、owner 锁、双传输、embed 入口、延迟基线）已可用，但 spawn 纪律与两个测试缺口必须在 Host 嵌入前收口；R9 的完整入口被既存 TS 类型错误挡住，不能用“Rust 455 例全过”代替。

H 线距离“Rust Host 是唯一 Host”仍有结构性差距：开关与回退仍在、窗口 host 仍是 Node utility process、205 个成员 Pending（含 bots/hooks/off-peak/plugin-sync 等真实 UI 面）、无崩溃隔离测试、Host 默认不做存量会话导入、无 replayable E2E 与无窗口调度。当前形态只能作为 dev 开关下的局部迁移预览，不能切默认。

---

### 证据边界

- H4/H5 的部分 oracle 与测试定位由本轮的只读子调查汇总，关键结论（settings 默认值、readdir locale 覆盖、tasks-index busy_timeout、Agent V4 真实客户端范围、Phase 2 调度缺口）均已回到源码/manual 命令现场复核。
- 复核（同日，HEAD `c15a22aa`）：重跑 `check-escode-host-boundaries.mjs`（exit 1，同两处 400 行）、`check-escode-cli-rust-boundaries.mjs`（exit 0）、`generate-escode-host-service-inventory.mjs --check`（exit 0，46/539）、`prettier --check docs/specs/bots-v4-stream.md`（不符）；现场重读 H2.1 回退（`rustHostManager.ts:101`、`rustHostBridge.ts:38`、`index.ts:836-838`）、H2.3（`desktopHostProcess.ts:36,737`）、H1.5（`connection.rs` 239 行内无测试）、H3.3（`tasks_index/mod.rs:115` 为 25）、settings 回退 `{}`、R5 裸 `spawn_blocking` 清单、`rustHostRpc.test.ts:62`、`parseCpuTime`、CI `:7`/`:148`，均与正文一致。复核修订：H 计数、H1.3/H1.6 下调、R 线硬性条件、§6 新增 `rust-u1-runtime-lib.md:80` 漂移、§9.3 Host 进度对照。
- 未执行：Electron App 进程树采集（无图形会话）、安装包构建与内容核对（无 dist/打包环境）、`cargo-deny`（未安装）、真实性能 A/B（harness 自测失败且无图形会话）。
