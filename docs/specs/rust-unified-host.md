# Rust 统一 Host 迁移计划（对标 ZCode-rs 并超越）

> 状态：草案 · 2026-10-14
> 关联：`rust-cli-parity-plan.md`、`rust-packaging.md`、`rust-release-rollback.md`、`rust-runtime-performance.md`、`apps/escode-cli-rust/CONTRACT.md`、实现质量对比：`docs/reports/rust-impl-quality-comparison-2026-10-14.md`
> 子 spec 约定：每期开工前先写 `docs/specs/rust-uX-*.md`（与对标仓库同构命名，便于交叉对照）；每期的验收场景随子 spec 定义，本文只定分期边界与总验收。

## 1. 背景与目标

对比结论（2026-10-14 分析）：

- **对方**：ZCode-rs 已把 CLI（10.7 万行）与桌面 Host（14.4 万行）都迁到 Rust，并把 runtime 嵌入 Host 进程（每 workspace 一个实例、懒重启、双传输）。
- **其 App 级实测**：空闲 436 MiB、每多一个 workspace 3.7 MiB、冷启动 2.01 s、首段 236 ms、退出 0.28 s；但**生产默认仍是 Node Host + TS CLI**（切换排在 U8，未开始）。
- **我方优势**：在"已出厂"——Rust CLI 已默认（2026-10-03），回退工程完整（只读导入、owner 锁、显式不静默回退、installed 四平台演练）。
- **我方差距**：Host 仍是 TS/Electron、每 workspace 一个 Rust CLI 子进程，App 级收益只吃到了子进程那一段。

本计划目标：**在空闲内存、每 workspace 内存、冷启动、首段、退出、控制延迟六个维度对标并超过 ZCode-rs 的实测值**，同时保持与 3.15.1 行为严格对齐、每阶段可交付可回退。

决策记录（2026-10-14 已确认）：

1. **路线**：分阶段——先 runtime 库化（embed 入口、显式环境、多实例、同进程重启、双传输），再分域迁移 Host；不做一次性大爆炸。
2. **口径**：先建 App 级 A/B harness 拿真实基线，再把超越值写死进计划（§5 为基线前初始主张）。
3. **护栏**：全套对标 + 渐进清零（toolchain pin、clippy deny 清单、deny.toml、nextest、unwrap/expect 按 crate 清零后逐 crate deny、任务与通道纪律脚本）。
4. **超越项（四项全选）**：进程内免序列化直连、跨实例共享缓存、启动/退出路径优化、严格等价测试网。

非目标：TUI（维持 `-p`，`crates/tui` 维持契约 stub）；npm 启动器式分发（我方控制安装包）；Renderer 可见协议语义变更；无兜底的一次性单进程切换。

## 2. 目标架构与状态所有者

```
┌─ Electron Main（窗口 / 原生 / 进程拉起与转发，不承载业务状态）────────────┐
│                                                                           │
│   ┌─ Rust Host（apps/escode-host，每个 App 一个进程）───────────────────┐ │
│   │  Gateway（窗口连接注册表：到达序 lane、字节水位出站队列、关闭原因） │ │
│   │  Services（settings / sessions+tasks / workspace / providers /      │ │
│   │            extensions / account / automation / remote）             │ │
│   │  runtime-link（Host 中唯一认识 runtime 的 crate）                   │ │
│   │    ├─ embedded：内存 Link ←→ runtime 实例（每 workspace 一实例，    │ │
│   │    │            同进程 tokio task，阶段 4 起免序列化直连）          │ │
│   │    └─ child   ：stdio Link ←→ escode-cli-rust 子进程                │ │
│   └─────────────────────────────────────────────────────────────────────┘ │
└───────────────────────────────────────────────────────────────────────────┘
Renderer（TS，不变）── WebSocket / RPC（capability 握手）──→ Rust Host
```

状态所有者：

- **runtime**：会话事实、输入队列、投递（topic 订阅/快照恢复）、模型请求。
- **Host**：实例表、代次、订阅归属、调度（cron/offpeak）、设置写入（唯一写者）。
- **Main**：窗口、原生操作、进程拉起与转发。

runtime 实例生命周期（事件顺序）：

```
get_client(ws) ──[槽位 single-flight]──► spawn gen N+1 ──► available
   ▲                                                          │
   │  崩溃/退出 ──► unavailable ──（懒）下次 get_client 才 spawn gen N+2
   │                                                          │
   └─ cancel ──► drain（工具进程树回收）──► 超 wind-down 上限 ──► abort + 记录残余任务
```

边界与幂等：

- 双传输之上只有一份 App 协议（V4 帧 + runtime→Host 请求/通知）；链路之上代码不区分传输。
- 每代实例 = 一个 generation；懒重启；一代结束后的输入结算必须有明确语义（已接受输入持久化、排队输入失败码、可重发）。
- workspace owner 锁在"同进程多实例"下的语义要重做：现有以 pid 标识的磁盘锁在嵌入模式下 pid 相同，需引入进程内唯一 token + 进程级登记表（对标仓库 U1 §6.2 已给出验证过的做法）。
- 双运行期数据互操作：Rust Host 写入的任何共享状态（settings.json、tasks-index.sqlite、会话库）必须与 Node Host **双向可读**，否则 dev 开关来回切会破坏状态。三条落点：
  - 同文件同格式、跨进程文件锁；
  - DDL 字节冻结 + checksum；
  - `BEGIN IMMEDIATE`（busy_timeout 5000ms）。
- 对标仓库冻结了格式（其 settings/tasks-index 设计），但没有交替写读的 E2E 演练——我方补上即算超越。
- Host 作用域：现文档为 window-scoped Local Host；目标形态是"一个 App 一个 Host"（所有窗口经 attachment 接入）。此边界变更在阶段 2 子 spec 立项时确认，并同步更新 AGENTS.md 与相关 spec。

## 3. 分期计划

### 阶段 0：护栏与测量基线（估算 1–2 周）

交付物：

- [x] `rust-toolchain.toml` pin 1.95.0（已完成；`rust-runtime-ci.yml` 三平台同步 `toolchain: 1.95.0`）
- [x] workspace `[lints.clippy]` deny 清单 + `clippy.toml`（已完成子集：`manual_*`/`needless_*`/`redundant_*`/`unnecessary_*` 等 21 条 + 测试放宽；`unwrap_used`/`expect_used`/`undocumented_unsafe_blocks`/`disallowed_methods` 按 crate 清零后渐进加入）
- [x] `deny.toml`（已完成：advisories 无 ignore、licenses 14 种白名单、bans `multiple-versions=warn` + `wildcards=deny`（`allow-wildcard-paths` 仅放行工作区内部 path 依赖）、sources 只允许 crates.io；`cargo deny check` 四类全过；CI checks job 接入 `EmbarkStudios/cargo-deny-action`，本地 `pnpm check:escode-cli-rust:deny`）
- [x] nextest 配置（已完成：`apps/escode-cli-rust/.config/nextest.toml`——慢测试 30s 报告/60s 终止、retries=1、fail-fast=false、JUnit；本地实测 396 tests 全过；`scripts/test-escode-cli-rust.mjs` 首选 nextest、缺失时回退 cargo test 并提示；CI test job 经 `taiki-e/install-action@nextest` 三平台预装）
- [x] App 级 perf harness 骨架（2026-10-14 完成）：`scripts/bench-escode-app.mjs`
  - 三配置注册（A=Node Host+Node CLI / B=Node Host+Rust CLI / C=Rust Host+内嵌 runtime 为阶段 2 占位、显式报错）；交错 N 轮取中位数。
  - 双口径内存（Windows WorkingSet64 + PrivateMemorySize64；POSIX 走 RSS，macOS footprint 待接入）；按角色分解（main/renderer/gpu/utility/agent/agent-node）。
  - 每轮隔离 data dir（A/B 交替写同一 data dir 会共享会话并触发两套存储格式互相迁移，必须隔离）；不建立调试连接；`--reuse-profile` 显式 opt-in 共享 profile。
  - `--self-test` 用合成进程树验证发现/采样/交错/中位数/杀树全链路（实测 3 轮交错 PASS；夹具教训：合成 Buffer 必须被闭包持有，否则分配后无引用的 buffer 会被 V8 外部内存 GC 回收，读数随 GC 时机倒挂）。
  - **真实 A/B 基线已跑**（3 轮交错，2026-10-14），报告 `docs/reports/rust-app-perf-baseline-2026-10-14.md`：App 口径（electron 主进程子树）A 1419.6 / B 1293.6 MiB 工作集、私有提交 1102.2 / 974.5 MiB，B 不劣于 A。
  - 就绪判定修正为「electron 主进程 + renderer 按进程名在场」（旧判定被 dev 的 concurrently 参数含 "electron" 误触发，实测 electron 在 dev 约 t≈173 s 才出现，前三次候选轮数据作废）。
  - §5 八项指标逐项记录，未覆盖 5 项的接入条件已列（多 workspace / 首段 / 优雅退出 / 进程内 RPC / 4 路流式）；三平台 CI 维持 CLI 级 `bench-escode-cli-node-rust.mjs`（Electron 需图形会话，App 级 CI 接入待阶段 2 后评估虚拟显示方案）。
- [x] unwrap/expect 基线快照（按 crate 计数，作为渐进清零的进度表）——`docs/reports/rust-unwrap-baseline-2026-10-14.md`
- [x] 修红完成：全部越界拆分（历史 3 处 + fmt 折行新增 14 处，均为模块外迁）；boundary/fmt/clippy/全工作区测试全绿；附带修复 wiki 的 Windows `\\?\` realpath 历史 bug；`docs/rust-migration.md` 过期表述已更新
- [x] 差分基础设施垂直切片（2026-10-14 完成）：`scripts/diff-escode-cli-rust-parity.mjs`（+ `-capture`/`-diff`/`-report` 模块）打通「Node 捕获 → golden → 双跑差分 → evidence 报告」全链路。
  - 场景 `session-baseline`：capabilities → createSession → subscribe → sendText → 静默期关闭；双侧同一 mock SSE/临时 HOME/workspace/固定输入。
  - 对拍面：消息帧（L4，位置严格）/ 存储进度（L2 集合）/ 事件流（L3 集合）/ 模型请求（L4，字符串值字节级）/ workspace 文件（L5）。
  - 产出 evidence.json + report.md + 双侧原始与归一帧；差异三分类（已批准登记表 reason+removal / open 已登记未修复 / 欠账基线指纹棘轮拦新差异）。
  - 负向验证通过（移除一条指纹即判失败）；CI 已接入 bench job 三平台（报告进 job summary、artifact 上传）。
  - **观察期**：基线指纹此前只在本机 Windows 验证过稳定，三平台首次全绿后去掉 `continue-on-error` 转阻塞。
- [x] 我方差异账本第一轮（2026-10-14 完成）：`docs/reports/rust-difference-ledger-2026-10-14.md`
  - 分类 M/T/C/S/A/F；逐条给出定位字段与移除条件。
  - 真偏移：mcpTelemetry 缺发、模型请求字段名 `max_tokens`/`max_completion_tokens`、`tool_choice` 省略、`memoryEnabled` 缺字段、偏好请求时序、seq 编号/状态载体。
  - 已批准差异：capabilities 扩展、存储进度粒度、openTiming 诊断块、标题请求流式形态。
  - 正面结论：workspace 文件副作用两侧一致。
- [x] P0#1 catch_panic 完成：core-api 新增 `catch_panic`/`Panicked`，engine 循环与 run 任务两处接入；`a_panicking_run_still_reaches_a_terminal_phase` 红/绿验证（修复前挂死 5s 超时、修复后通过）
- [x] P0#3 进程树三补丁完成：
  - `host::process_group::isolate`（setsid，替换 4 处 `process_group(0)`）；
  - `process_tree::fall_back`（快照失败退路，两个调用点路由）；
  - `GroupKillOnDrop`（任务中止整组回收，带 `id().is_none()` 防 pid 复用）。
  - 新增 `fall_back_signals_the_root_group_without_a_process_table` 与 `isolate_starts_a_new_session_and_drop_kills_the_whole_group` 用例；WSL（linux）验证 unix 路径 + 既有 `shell_lifecycle` 4 用例全过；Windows 门禁与全量测试全绿。
- [x] P0#4 unwrap 热点完成：`registry.rs`（7 处）与 `mcp_hub.rs`（15 处）锁 unwrap 清零（poison 容忍，读路径不因写路径 panic 打崩）；基线快照 `docs/reports/rust-unwrap-baseline-2026-10-14.md`（生产 313 unwrap + 47 expect，core 157 为最大头，清零顺序已定）
- [x] 修复（阶段 0 实测发现的崩溃级缺陷）：Windows 主线程默认 1MB 栈在 `workflow-task-output` 场景的深层异步调用链下栈溢出（0xC00000FD，debug 构建，确定性复现）；`src/main.rs` 入口改为显式 16MB 栈线程上运行 tokio runtime（红/绿：修复前差分用例必崩、修复后通过）
- [x] 全套件验收（2026-10-14）：`pnpm test:escode-cli-rust`（含 163 个差分文件）在 CI 口径 env 下本地全绿（407 用例 / 0 失败 / 9 环境性跳过，44 分钟串行）。同批修复：陈旧 Node bundle（`plugins-uninstall` 误红）、`recovery` 退出码断言对齐 75 契约、`ESCODE_TEST_EXIT_MS=15000` 关闭预算加固（写入 CI env 与脚本注释）
- [x] P0#2 投递层测试完成：`subscription_delivery_snapshot_pause_resume_close_and_replace` 覆盖 ACK 先于首帧、重复订阅替换（旧 id 不可用）、conversation 续传（resume+deltas）、saturated 期间不投递、drained 补发快照、closed 清理与契约错误——此前 `subscriptions.rs` 零测试；Windows 与 WSL（linux）双平台通过
- [x] **P0 四项全部完成**（catch_panic / 投递层测试 / 进程树三补丁 / unwrap 热点）

验收：`pnpm check:escode-cli-rust` 与三平台 CI 全绿；基线报告已产出——`docs/reports/rust-app-perf-baseline-2026-10-14.md`（§5 全部指标逐项记录；可测项已数值化，未覆盖 5 项写明接入条件）。

### 阶段 1：runtime 库化（估算 2–4 周）

交付物（2026-10-14 全部完成，子 spec `rust-u1-runtime-lib.md`，验收报告 `docs/reports/rust-u1-runtime-library-2026-10-14.md`）：

- [x] `embed::spawn(RuntimeOptions { home, env, argv, cwd, workspace_identity, data_dir, config }) -> Embedded { io, task, cancel, tasks }` 公共入口（形态对标 `apps/zcode-cli-rust/src/embed.rs`；`config` 为清单外必要补充——见子 spec §2）
- [x] 显式环境：crates/ 非测试代码 0 处 `std::env::var*` / `current_dir` / `current_exe` / `args`（边界脚本新规则，唯一豁免 `core-api/src/env.rs`）；env 快照由组合根与 RuntimeOptions 传入
- [x] 多实例：同进程 2+ workspace 并发；owner 锁进程内登记表 + 实例 token（`WorkspaceOwnerGuard`）；同进程重启（关闭后再 spawn）正确
- [x] `Link` trait + 双传输：`StdioLink`（现有行为逐字节不变）+ `MemoryLink`（内存双工 + NDJSON 保等价，接口为阶段 4 免序列化预留）
- [x] 双传输一致性子测试：`tests/dual_transport.rs` 三用例——同一字节流两条链路产出同一 Input 序列（含无效 JSON/空白/无换行尾行）、输出帧字节等价、EOF 与关闭语义等价
- [x] 执行『实现质量对齐工作包』P1 结构对齐：逐项状态与细节见「实现质量对齐工作包」→「P1 结构对齐」的状态清单（#5/#6/#7/#10/#11 完成，#8/#9 ◐ 部分，#12 ⛔ 挂起）。
- [ ] #6 net 出口收拢、#9 plugins crate、#10 `v4/command` schema 校验、#11 bash registry 类型化：未做，随阶段 2 继续（各自独立验收）

验收：cwd/env 与 runtime 不一致时用 `RuntimeOptions` 的值；同进程两 workspace 并发无串扰；关闭后再 spawn 正常；环境读取检查为 0；双传输差分全绿；记录进程内控制延迟基线（p50 0.250 ms / p95 0.334 ms，n=50，MemoryLink NDJSON 路径）。全部达成，证据见验收报告。

**#12 遥测 ChunkWindow 的核查结论（2026-10-14）**：

- 现状：我方 pinned 协议对 `stream.chunk` 是 strict schema（`packages/shared/src/escode-protocol-v4/telemetry.ts:113-121`），字段只有 `channel/chunkLength/firstChunk/assistantMessageId/partId/parentToolCallId`；TS oracle（`conversation-telemetry-facts.ts:432`）每 delta 一条事实。
- 冲突：发送端做 250ms 合并并携带 `chunkCount/lastChunkAgeMs` 会同时违反协议契约与 §4.1 对齐口径（属"已批准差异"之外的偏移）。
- 结论：本项改为**条件项**——只有在协议 schema 与 TS 侧同时引入窗口语义（上游 3.15.1+ 或双方协调变更）后才实施；runtime 内部无第二消费方需要该聚合。

### 阶段 2：Rust Host 骨架与分域迁移（估算 6–10 周）

> **接手入口**：新会话从 `docs/reports/rust-unified-host-handoff-2026-10-15.md` 开始读——含代码地图、固定验证动作（门禁三连/负向验证/显式路径提交）、陷阱清单（共享工作树、二进制陈旧、夹具要求）、已登记差异与逐项下一步（含 TS 真相源的函数与行号）。

进度（2026-10-14，子 spec `rust-u2-host-skeleton.md`）：

- [x] **u2a 骨架与通路**：`apps/escode-host` 工作区完成——
  - host-core 有界通道/原子写；host-proto 契约清单 + 与 `ControlMessage` 的漂移门禁；
  - host-gateway 连接注册表/到达序 lane/capability 握手；host-control typed 控制消息；
  - host-runtime runtime-link 选择器（embedded/child）与实例表（single-flight、懒重启 generation、收口残余清单）；
  - 进程入口 attach/detach/转发/扇出/收口端到端。
- [x] **u2b 第一个服务域 settings**：与 TS Host 同文件同格式（顶层合并写、坏文件隔离、2 空格缩进）；双 Host 交替写读演练通过（真实 TS 设置服务 ↔ 真实 Host 进程，3 轮交替 + 未知键保留）。
- [x] **Renderer 直连的 RPC 面**：`@escode/rpc` 协议 + 真实客户端集成；`ESCODE_RUST_HOST` dev 开关与 Main 侧 attachment 切换（`rustHostManager.ts` / `rustHostBridge.ts`），见子 spec 实施进度表。
- [x] **CI 接入**：`pnpm check:escode-host`（checks 作业）与 `pnpm test:escode-host`（三平台 test 作业）。
- [x] **验收项「Host 服务契约清单（channel/method）`--check` 与源码一致」已机器化**：channel 方法表即分派来源（`Channel{name, methods}`），`host-rpc/tests/contract.rs` 双向对账清单与源码（Implemented 必须在表里、Pending 不得在表里、注册 channel ⊆ 清单），负向验证见子 spec。
- [x] **优先序 4–5 已落地部分**：
  - `escode-session` 读/写/预热/展示面；
  - `system.info` 与 `file` 精确子集（stat/readdir/checkFilesExist/resolvePath/readFileRange/readTextFile/预览两法/工作区三法）；
  - tasks-index 只读切片（生成器提取 TS 迁移账本与物理 DDL + 账本三分类对账 + `escode-task.getTaskMeta` + 真实 TS Repo 逐字段差分，含 `listTaskList` 全文搜索与分组结构读），见 `rust-u2-tasks-index.md`；
  - **`window-controller` 本地子集 8/8**（订阅面 + `listTaskList` + `mutateTask` + 归档删除；复用 tasks-index / syncer 会话摘要 / `WorkspaceEvents` 总线；TS oracle 帧差分 + 三处负向验证），见 `rust-u2-window-controller.md`——手机远控的时间线/置顶/归档/全局搜索不再空列表。
- [ ] **桌面窗口自身 host 的进程无关化**（`desktopHostProcess.ts:762`）：需广播/实时总线等子系统接受非 utility-process 的 window host。
- [ ] **优先序 4–9 其余服务域**：各域进度见对应子 spec 的进度表（`git` 17/18、`terminal` T1、`file-watcher`、`providers` P1 已交付；extensions / automation / remote 未开始）。

工作区结构：新增 `apps/escode-host`（独立 Cargo workspace），共用基础代码放独立 shared workspace（对标三 workspace 结构，理由：Host 不拖 CLI 编译与依赖）。注意现有 `apps/escode-cli-rust/crates/host` 是 CLI 的配置/路径 adapter，与新 Host 概念重名，子 spec 中明确更名或文档注明。

基础设施（先行）：TaskScope（`tokio::spawn` 唯一入口、panic 转错误事件、关闭时列出残余任务）、有界通道（每队列写明溢出策略）、Gateway（连接注册表、到达序 lane——登记必须在请求到达时同步完成，不能依赖任务调度顺序）、Main↔Host control 消息（stdio，日志与控制同一有序队列）、WebSocket + capability 握手。

服务移植优先序：

| 顺序 | 服务域                                                           | 理由                    |
| ---- | ---------------------------------------------------------------- | ----------------------- |
| 1    | 基础 crate（task scope / 通道 / 进程回收 / 原子写）              | 所有后续服务的底座      |
| 2    | Gateway + control + rpc                                          | 先立通路                |
| 3    | settings                                                         | 单写者收敛，小且高频    |
| 4    | workspace（git / terminal / watcher / 媒体预览）                 | 交互热路径              |
| 5    | sessions + tasks + tasks_index                                   | 业务核心，最大块        |
| 6    | providers + account                                              | 配置与鉴权              |
| 7    | extensions（skills / plugins / hooks / mcp / commands / memory） | 文件系统视图，可后置    |
| 8    | automation（cron / offpeak / bots）                              | 调度移入 Host           |
| 9    | remote + runtime-link 选择器                                     | SSH/WSL/Docker 与回退链 |

runtime-link：embedded 为默认（dev 开关整体控制 Rust Host），child 模式保留（差分、回退、故障隔离选项）；懒重启 generation 计数；wind-down 预算（工具进程树 TERM→KILL 收口上限）。

验收：

- Rust Host 在 dev 开关下跑通全部桌面 E2E（embedded 与 child 双链路）；
- 多窗口、无窗口调度、设置写入、崩溃重启；
- **双 Host 交替写读演练**（同一 settings.json / tasks-index 等共享状态在 Node Host 与 Rust Host 间往返写读，dev 开关来回切换不破坏状态）；
- 进程表断言（Rust Host 下无 Node utility / CLI 残留）；
- Host 服务契约清单（channel/method）`--check` 与源码一致。

### 阶段 3：切默认与清理（估算 2–3 周）

- [ ] 默认启用 Rust Host；安装包不再含 Node Host 与 TS CLI bundle
- [ ] 六目标打包与签名（沿用 `rust-packaging.md` 的 6 目标 + macOS 签名开关）
- [ ] installed 作业扩展：四平台 Node→Rust 切换真机演练 + 回退演练
- [ ] golden freeze：差分套件从"双跑 Node"切换为"goldens + spec"模式并全绿，之后才允许删除 Node Host
- [ ] 回退矩阵更新（`rust-release-rollback.md`）；AGENTS.md / CONTEXT.md 同步

验收：全平台 E2E 矩阵；冷启动、常驻内存、流式 CPU 全面不劣于当前组合，实测写入 `docs/reports/`。

### 阶段 4：超越项（与阶段 2–3 并行，收尾 +2 周）

1. **进程内免序列化直连**：`MemoryLink` 切换为类型化消息直传（不经 NDJSON/serde）；帧边界、flush 窗口、关闭语义与 stdio 规格同源；双传输一致性子测试守护。目标：进程内控制 RPC p50 ≤0.1 ms（对标仓库进程内仍是 duplex+NDJSON，p50 0.2–0.3 ms）。
2. **跨实例共享缓存**：provider 配置（FileStamp/ChangeGate 式变更检测）、模型注册表、skill/plugin 目录做成 Host 内共享的不可变快照 + 失效信号（对标仓库每实例各自缓存、extensions 每次调用重扫）。目标：每 workspace ≤2.5 MiB（其 3.7 MiB）。
3. **启动/退出路径优化**：Host 随 App 预热（与窗口创建并行）、非关键服务懒加载、退出快照式刷盘。目标：冷启动 ≤1.8 s、首段 ≤180 ms、退出 ≤0.2 s（其 2.01 s / 236 ms / 0.28 s）。
4. **严格等价测试网**（见 §4，贯穿全部阶段）。

### 实现质量对齐工作包（对比报告结论落地）

结论来源：`docs/reports/rust-impl-quality-comparison-2026-10-14.md`（按域双端读实现逐项对比，2026-10-14）。

**总判断**：多数实现质量差距是同源代码中对方后续修复与重构未同步（B2/B3/B7、TaskScope、ChunkWindow、FileStamp 等可直接移植）；我方另有 10+ 处对方没有的实现（见保留清单）。原则：**先修正确性债（P0），再做结构对齐（P1），Host 迁移不得建立在未修复的引擎上**。

| 域              | 判决                                                              | 落点               |
| --------------- | ----------------------------------------------------------------- | ------------------ |
| Bash 权限/解析  | Z 更好（registry 类型化、语料逐字段）                             | #11、P2            |
| Engine 核心循环 | Z 更好（差异最致命：无 catch_panic，run panic 挂死）              | #1、#5、#8、#12    |
| 存储层          | Z 更好（结构化错误、对照 corpus）；锁层面我方更好                 | P2；保留 owner 锁  |
| 工具层          | 分裂：Edit 语料/进程树/原子写 Z 好；Read/搜索/代码页/文件锁我方好 | #3、P2；见保留清单 |
| 模型/网络       | Z 更好（net 单入口、配置缓存、失败现场字段）                      | #6、#7、P2         |
| 协议/投递       | Z 更好（delivery 状态机、水位闭环）；我方有 EOF 看门狗与键序保留  | #2、#10、P2        |
| 扩展体系        | 分裂：插件/hooks Z 好；子代理/cron 我方好                         | #9、P2；见保留清单 |

#### P0 正确性（并入阶段 0，立即执行）

| #   | 改哪里                                                                                      | 怎么改                                                                                                                                                                                                                         | 验收                                                                                       |
| --- | ------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------ |
| 1   | `crates/core/src/app/engine.rs` 主循环 + `crates/core/src/app/run.rs` 的 run 任务           | 引入 `catch_panic`（对标 `rust/crates/async`）：两处把 panic 转成失败终态事件，保证 `Finished` 必达；收尾统计残余任务并 warn；`Panicked` 与 `StorageCommitFailure` 同等视为"此后不得提交"（对标其 `engine_shutdown.rs:15-17`） | 新用例 `a_panicking_run_still_reaches_a_terminal_phase`；EOF/close 在 run panic 后不再挂起 |
| 2   | `crates/core/src/app/subscriptions.rs`（342 行，零测试）                                    | 测试先行：建假 sink harness（对标其 `crates/delivery/tests/delivery/support.rs`），覆盖 ACK 先于首帧、窗口合并、拥塞丢缓冲→快照恢复、重复订阅替换、关闭连接后在途 topicOpen 回复不登记                                         | 五类行为测试全绿；为 P1 拆投递状态机铺路                                                   |
| 3   | `crates/tools/src/process_tree.rs:201`、`crates/tools/src/tool_process.rs`                  | snapshot 失败 fallback 对根进程组 killpg；Bash/hook 子进程加 setsid；任务 abort 时 Drop 对整个组 SIGKILL（对标其 `proc/tree/posix.rs:173-204`、`group.rs`、`ledger.rs:292-313`）                                               | `tests/shell_lifecycle.rs` 扩用例：ps 失败仍能终止；/dev/tty 场景不挂起                    |
| 4   | `crates/model/src/registry.rs`（7 处 unwrap）、`crates/tools/src/mcp_hub.rs`（锁 unwrap）等 | 按本文件 §4.6 欠账清单逐处清理：模型入参直取改 `ok_or` + context；锁 unwrap 改 poison 容忍或 `unreachable!` + 说明                                                                                                             | unwrap/expect 计数下降并记入基线快照                                                       |

#### P1 结构对齐（并入阶段 1，与库化并行；#5 是嵌入多实例与收口的前置）

状态（2026-10-14）：

- [x] **#5 spawn 收拢**：完成——`core-api/src/scope.rs`（Spawner 唯一入口、panic 上报、`running()` 残余清单、`drain` 宽限、实例 env 传播；core/tools/model/state 全部 spawn 点收拢）。
- [x] **#6 网络出口单入口**：完成——`crates/net`：Purpose 三 client、代理/CA 单一解析、rustls provider 归位、clippy `disallowed_methods=deny` 强制；api_key/headers 的 RedactedString 同批落地。
- [x] **#7 FileStamp/ChangeGate**：完成——`host/src/file_stamp.rs`（五类指纹测试）+ registry 闸门（四测试）。
- [ ] **#8 分派/所有权/waiter**：◐ 部分——`CoreRuntime` 第二所有权已删；waiter 三表合并单注册表 `Waiters::release/clear`（+ 两测试）；typed 方法分派与瞬态 `Submission` 未做。
- [ ] **#9 插件域**：◐ 部分——SKILL.md 扫描合并单点 `plugin_skill_files.rs`、插件错误类型化 `plugin_failure.rs`（两处文案匹配收敛为 downcast，TS code 不变）已完成；plugins crate 抽离与 `Ports` 注入待做。
- [x] **#10 `v4/command` schema 校验**：完成。
  - `scripts/generate-escode-cli-rust-command-schema.mjs` 从 TS zod 生成 `command_schema.json`（35 命令类型 + 信封 + CAS 规则集合）；
  - `crates/protocol/src/json_schema.rs` 子集校验器 + `command_schema.rs` 对齐 TS `parseCommandEnvelope`；
  - 漂移双向门禁：生成器 `--check`（已入差分套件）+ Rust 侧信封字段对账测试；协议入口记录违规为 debug 日志，线上应答不变。
- [x] **#11 bash registry / policy 类型化**：完成。
  - `bash_callbacks` 声明表 + 双向对账；`safeFlags` kind 枚举；
  - 策略表 serde `deny_unknown_fields`（`bash_policy_tables.rs`，未知字段/未知 kind 加载期失败）；
  - registry 类型化 `HashMap<String,Node>` + `version`/`hash` 断言（`bash_registry.rs`，生成器输出规范 JSON 的 sha256）；
  - 余下「全量逐字段语料 + 种子化 fuzz」属 P2 语料项。
- [ ] **#12 遥测 ChunkWindow**：⛔ 挂起——与协议 strict schema / TS oracle 冲突，见阶段 1 注记。

| #   | 改哪里                                                                                                               | 怎么改                                                                                                                                                                                               | 参考（对标仓库）                                                           |
| --- | -------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------- |
| 5   | 全 app 46 处 `tokio::spawn`（core 内全部 detached）                                                                  | 最小 TaskScope/Spawner：唯一 spawn 入口、panic 上报、`running()` 残余清单、关闭宽限；实例级测试断言 `running()` 为空                                                                                 | `rust/crates/async/src/scope.rs`、`tests/suite/embed.rs:318-326`           |
| 6   | 网络出口：`crates/model/src/provider.rs:49-87`、`crates/tools/src/mcp_http.rs`、`crates/tools/src/web_fetch_http.rs` | 抽 `crates/net`：Purpose 三 client、代理/CA 规则、identity 头、gateway 端点改写、子进程 env；删 model crate 6 处 `std::env::var*`；api_key/headers 改 RedactedString；加 clippy `disallowed_methods` | `crates/net/src/lib.rs`、`rust/crates/kit/src/secret.rs`                   |
| 7   | `crates/model/src/registry.rs` 每秒真读+SHA256                                                                       | FileStamp/ChangeGate（len/mtime/inode/ctime + 60s 兜底全读）+ 五类稳定性测试（出现/改写/原子替换/删除/账号旁路）                                                                                     | `rust/crates/kit/src/fs/stamp.rs`、`registry_tests.rs:70-123`              |
| 8   | 字符串方法分派、`crates/core/src/runtime.rs`（`CoreRuntime` 第二所有权）、waiter 三张表、持久化瞬态选项              | 分派改 typed；`CoreRuntime` 标注仅嵌入/测试或删除；waiter 合并单注册表 `Waiters::release(owner)`；瞬态选项改内存 `Submission` + `strip_transient`（不再写进 intent 再反查）                          | `core-api/src/transport.rs`、`app/waiters.rs:130-146`、`app/submission.rs` |
| 9   | 插件域（tools 内约 40 个 `plugin_*.rs`）                                                                             | 抽 `crates/plugins` + `Ports` 注入；错误 enum 化（替代 `plugin_install.rs:288-311` 文案前缀匹配）；SKILL.md 三处扫描合并单点                                                                         | `crates/plugins/src/{ports,failure,components}.rs`                         |
| 10  | `v4/command` 校验（现为 `engine_requests.rs:82-91` 粗拒绝）                                                          | 引入 schema 校验：子集 json_schema 校验器 + 从 TS zod 生成 schema + 漂移测试                                                                                                                         | `crates/protocol/src/{json_schema,command_schema}.rs`                      |
| 11  | `crates/domain/src/bash_rule_prefix.rs:73-194`（Value DOM 下标）、`bash_policy.rs:31-47`                             | registry 改 typed `HashMap<String,Node>` + version/hash 断言；policy enum + `deny_unknown_fields`；callback 名字集合双向对账（替代 `_ => true` 静默）                                                | `crates/bash/src/{registry,tables}.rs`                                     |
| 12  | 遥测 `Event::Text` 出口（现每 delta 一条 fact）                                                                      | 接 ChunkWindow（250ms、携带 chunkCount/lastChunkAgeMs、firstChunk 直发、非 chunk 事实先 flush 保序）                                                                                                 | `crates/domain/src/telemetry/chunks.rs`                                    |

#### P2 语料与打磨（并入阶段 2 / 持续）

- bash 语料全量逐字段化 + 种子化 fuzz；edit-match 用 TS oracle fixture 替代 100 行手写单测（对标其 137KB `edit-match.json`）；补 `.ipynb` 拒绝与 CRLF 多数表决。
- 存储：结构化启动错误（`code`/`sqlite_code` 分类）；`rust_*`/`dwf_*` DDL checksum 哨兵；导入对照 corpus（含 rewind/shared/attachments 分支）；列表路径去掉 TS 回填 IO；rowsRange 走 SQL 尾页。
- 进程孤儿账本 + 启动 sweep；原子写 rename 重试 + 父目录 fsync；MCP 进程账本/遥测；`ModelStatus` 事件加厚（脱敏头、providerKind、异常类型、timings）；Rust 进程内 mock SSE。
- 投递层拆出独立状态机（recovering 语义、pin 释放、载体感知分片预算 + 分片上限、输出 backlog 高低水位滞回）。
- 确定性约定（固定 Clock、确定性 UUID、realpath、固定 SHELL）写入 Host 测试规范；插件安装副作用外移 mutations。

#### 保留清单（对齐时不得抄丢）

owner 锁 + exit 75、跨进程 `file_lock.rs`、Windows 代码页解码、credential_cipher、OAuth 全套、MCP inputSchema 键序捕获（`mcp_raw_capture`）、model-IO JSONL 轨迹、EOF 停滞看门狗、单管道输出保序、embedded-search prelude、Read 重复读去重桩、子代理权限模式（auto/plan/Explore）、cron/offpeak CLI 语义 + corpus。

## 4. 严格等价测试网（贯穿）

"没有偏移"必须是可证伪的：把每一个被替换的实现（现 Node Host、原 TS CLI、原始 App 行为）当 **oracle 逐字节对拍**，而不是当参考文档。

### 4.1 先定义判据：差异账本

必须逐字节/逐字符一致的面（任何偏差都按 bug 处理）：

| 面                                                                        | 对照物（oracle）     | 现状资产                                                                                                                                                                                                                                                                                                                                                                                                                                |
| ------------------------------------------------------------------------- | -------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Renderer↔Host RPC：方法名、参数、结果形态、事件名与因果顺序、错误码与文案 | Node Host            | 需新建（rpc fixtures 由 TS 侧生成）                                                                                                                                                                                                                                                                                                                                                                                                     |
| Host↔runtime：V4 帧（NDJSON 字节）                                        | Node Host / Rust CLI | 已有生成器与差分；垂直切片实测修正口径：**全字节级不可达**——Rust serde_json 的 Map 按键排序（BTreeMap）与 TS 插入序不同，且进度/遥测流条数随实现与冲刷时机不同（见 `docs/reports/rust-difference-ledger-2026-10-14.md`）。帧面按 L4（有序全字段，键序归一登记在 evidence）对拍 + 字节级差异行数作诊断；计数型流（storageProgress/streams）按集合级 L2/L3 对拍并另记时序事实。任何消费方依赖键序或帧字节冻结时，须先做显式序列化顺序对齐 |
| 模型可见面：三协议请求体、工具 schema 与描述（字节级）、tool_result 全文  | 原 TS CLI            | 已有字节级门禁 + 176 个差分测试；垂直切片补充请求体逐字段对拍（字符串值字节级）                                                                                                                                                                                                                                                                                                                                                         |
| 持久化与副作用：SQLite 语义、设置文件、插件/skill 文件布局、hook 信任锁   | Node Host            | 部分已有（存储对照类脚本）；垂直切片已记录两侧存储文件级事实，行级/跨读演练在阶段 2                                                                                                                                                                                                                                                                                                                                                     |

允许不同，但必须登记理由与移除条件（作为 review 检查项）：传输序列化方式（内存 Link 免序列化）、缓存策略、任务实现、进程/线程模型、时延与内存、按新 spec 定义的日志格式。

明确禁止的"近似"手段：trim、合并消息、工具排序、只比 JSON 等价、事件重排（对标仓库 audit 把其自身差分器的 `canonicalOrder` 列为缺陷，我方不沿用）。

### 4.2 捕获原始事实：golden 管线

- 逐场景捕获未归一的原始事实：RPC 帧、事件序列、模型请求、工具结果、存储快照、文件副作用。
- 确定性环境：临时 HOME、固定 SQLite、mock provider（SSE 逐块放行）、固定 stdio MCP、假时钟。
- **已落地（垂直切片，阶段 0）**：`scripts/diff-escode-cli-rust-parity.mjs`
  - 捕获层（`-capture`）：同一 mock SSE、临时 HOME/workspace/storage、固定 clientId/commandId/文本、400ms 尾帧静默期。
  - 差分层（`-diff`）：归一化规则随 evidence 输出、首差异 UTF-16 位置/JSON 路径/双侧 sha256、指纹棘轮。
  - 落盘层（`-report`）：evidence.json + 双侧原始/归一帧 + 模型请求 + report.md。
  - 登记表 `…approved.json`（reason+removal，双侧钉住匹配）与欠账基线 `…baseline.json`；示例场景 `session-baseline`；`--strict` 在欠账清零后启用。
- 生成器 + `--check` 防漂移（复用现有 36 个 `generate-escode-cli-rust-*.mjs` 与 26 份语料的机制）。
- **golden freeze（阶段 3 前置）**：删除 Node 前，把全部 goldens 冻结为自包含资产；此后 oracle 从"双跑 Node"切换为"goldens + spec"，必须能独立重验。

### 4.3 三层门禁（每次提交 + 三平台 CI）

- **L1 契约层**：协议 codec 字节级（fixtures 由 TS 生成）；工具 schema/描述字节级（已有）；contract inventory——Host 服务 channel/method 清单与源码一致性 `--check`，未实现成员立即回 `NotImplemented`（Renderer 不挂起，缺口显式可见）。
- **L2 行为层**：Node Host vs Rust Host 场景双跑差分（事件因果序不允许重排）；持久化跨读（Rust 读 Node 写的库、反向亦然）；双传输一致性子测试（全 scenario 在 stdio / 内存两链路双跑）；双运行时差分（Rust CLI 子进程 vs 嵌入实例）。
- **L3 端到端**：桌面 E2E 双 Host 跑同一场景集（embedded + child 双链路）；同时验证 `desktop-continuous` 与 `web-remote-replayable` 两种链路语义（AGENTS.md 约束）及手机远控 attachment 复用场景；进程表断言（无 Node 残留）；崩溃注入。

### 4.4 差分器纪律与闭环

- 报告首个不同字节、UTF-16 位置、JSON 路径、消息序号、SHA-256 与双侧上下文。
- 差分器自身的任何归一化步骤必须列出理由并被 review（见 §4.1 禁止清单）。
- 每个发现的偏移：分类（真偏移 / 已批准差异）→ 修复或记账 → 固化为永久回归用例；禁止"用例过时"式沉默修改。

### 4.5 CI 产物

每轮产出机器可读 evidence JSON + 人读报告（对标其 `rust-node-cli-parity-evidence-*.json` 形态）；按契约清单统计覆盖（有差分场景的 channel 方法占比），缺口清单进每期验收。注意对标仓库的教训：其 evidence JSON 至今**没有任何自动消费方**，其 fixture `--check` 门禁与 `test:rust-host` **不在 CI**（只本地跑）——我方所有门禁必须进 CI。

### 4.6 对拍松紧分级与欠账清单

对拍强度从高到低分五级，每个面必须声明所用级别；降级必须有理由并登记为 review 检查项：

| 级别          | 含义                               | 适用                                                                   |
| ------------- | ---------------------------------- | ---------------------------------------------------------------------- |
| L5 字节级     | 序列化后逐字节相等（含键序与转义） | 模型可见面、wire 帧、工具描述/schema、routed MCP inputSchema           |
| L4 有序全字段 | 归一化随机值后逐字段有序相等       | 事件序列、tool_result、RPC 帧                                          |
| L3 集合级     | 排序后多重集合相等                 | 仅限并发完成顺序天然无序的场景，且必须**另附因果序断言**               |
| L2 投影级     | 键集合 + 指定子集字段              | 仅限逐一列出被丢弃字段并登记理由                                       |
| L1 类型级     | 只比 typeof / 存在性               | 仅限时间戳与随机 ID 类，禁止用于语义字段（如 `revision`、`seq`、状态） |

当前欠账（2026-10-14 盘点，按序收紧）：

- [ ] `packages/services/tests/escode-cli-rust-telemetry-facts.test.ts:90-95`：事件顺序被排序多重集合抹平，与 `rust-cli-parity-plan.md:189`"事件因果不能抹平"冲突；拆成"内容相等 + 顺序相等"两条断言。
- [ ] `packages/services/tests/escode-cli-rust-tool-parity.test.ts:296-302,322-323`：删除 `gitDiff/perf/filePath/structuredPatch` 后再比；Write/Edit 结构化 diff 纳入对拍或登记为已批准差异。
- [ ] `packages/services/tests/escode-cli-rust-session-messages.test.ts:10-17`：该协议面零对拍覆盖，补形状对拍或显式记账。
- [ ] `packages/services/tests/escode-cli-rust-differential.test.ts:82-94`：行投影降级为键集合 + 8 字段 + typeof，逐项核对可恢复的字段。
- [ ] workflow 系列把数值时间/`revision` 归零的归一化（`-create-workflow.test.ts:150`、`-workflow-queries.test.ts:117`）：语义字段不得抹平。
- [ ] `-real-data-rehearsal.test.ts:24-30`：真实库演练只比计数，至少补内容哈希级对照。
- [x] 垂直切片的级别声明（2026-10-14）：消息帧 L4（键序归一已登记；位置严格）/ 存储进度 L2（阶段集合+键集合，条数为进度事实）/ 事件流 L3（方法×kind 集合，条数随冲刷时机，时序事实见 evidence.timing）/ 模型请求 L4（字符串值字节级）/ workspace 文件 L5。
- [x] 切片真偏移收敛进度（2026-10-14，详见 `docs/reports/rust-difference-ledger-2026-10-14.md`）：
      M 系列已修复（模型请求字段名 `max_completion_tokens` / `tool_choice:"auto"`）；C 系列部分收敛
      （`memoryEnabled` 进全部遥测事实与后续命令 ack；偏好请求提前到创建期发起）；剩余：`process/mcpTelemetry`
      缺发（消费方=ARMS 埋点，随阶段 2 P2 MCP 进程账本实现）、命令 createSession ack 字段与请求帧位置
      （已批准登记，含移除条件）、会话 seq 编号/状态载体（随阶段 2 投递层对齐）。清账后 `--strict` 接入 CI。

### 4.7 用对标仓库的审计分类做我方自查

对标仓库的逐字符审计（59 条差异，M/T/C/S/A/F 六系列）证明"生成资产门禁全绿 ≠ 轨迹对齐"（其 schema/fixture 全绿的同时仍查出 59 条真实差异）。阶段 0 对我方做一轮同型审计：以模型可见面优先（M/T/C 分类法），产出我方差异账本，带完整定位字段（首个不同字节、UTF-16 位置、JSON 路径、消息/事件序号、SHA-256），逐条分类"真偏移 / 已批准差异"。

**第一轮结果（2026-10-14）**：`docs/reports/rust-difference-ledger-2026-10-14.md`——垂直切片 `session-baseline` 场景实测：

- 正面结论：workspace 文件副作用两侧一致。
- 已批准 4 项：capabilities 能力位扩展、存储进度粒度、openTiming 诊断块、标题请求流式形态。
- 真偏移 6 类：`process/mcpTelemetry` 缺发、模型请求字段名 `max_completion_tokens`/`max_tokens`、`tool_choice` 显式/省略、命令响应 `memoryEnabled` 缺字段、偏好请求时序与 id 形态、会话 seq 编号/状态载体；其中模型可见面 2 类（M-1/M-2）按 M 优先收敛。
- 欠账 12 条由基线棘轮拦截新差异，`--strict` 在清账后启用。
- 另证伪一条早期口径：帧面"全字节级相等"不可达（serde_json BTreeMap 键序 + 进度/遥测条数差异），§4.1 已修正为 L4 + 键序归一登记 + 字节级诊断。

## 5. 指标承诺（基线前初始主张，拿到 §阶段 0 基线后校准并锁定）

| 指标                         | ZCode-rs 实测（M1 Max / macOS footprint） |               我方初始目标 | 测量方式                      |
| ---------------------------- | ----------------------------------------: | -------------------------: | ----------------------------- |
| 空闲内存（全 App）           |                                   436 MiB | ≤430 MiB（按本机口径校准） | App harness，双口径           |
| 每多一个 workspace           |                                   3.7 MiB |                   ≤2.5 MiB | N=1/4/8 交错中位数            |
| 冷启动到主界面可用           |                                    2.01 s |                     ≤1.8 s | harness                       |
| 首条消息第一段               |                                    236 ms |                    ≤180 ms | mock SSE + 真实 provider 双测 |
| 退出（8 workspace 全部结束） |                                    0.28 s |                     ≤0.2 s | harness                       |
| 进程内控制 RPC p50           |                                0.2–0.3 ms |    ≤0.1 ms（免序列化直连） | 双链路 latency 场景           |
| 4 路流式全进程 CPU           |                                     39.2% |                       ≤35% | 剥离调试连接后采样            |
| 4 路流式 Host+network CPU    |                             约 5 个百分点 |                ≤4 个百分点 | 分进程采样                    |

## 6. 风险与对策

| 风险                                                   | 影响                                        | 对策                                                                                                               |
| ------------------------------------------------------ | ------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ |
| Host 体量（14 万行量级移植）                           | 周期长、回归面大                            | 分域交付 + dev 开关 + 与 TS Host 全量差分；优先序按热度；对标仓库即已验证的参考实现，设计期大幅压缩                |
| 单进程爆炸半径                                         | 一个 workspace 崩溃波及全部                 | per-instance Engine panic 重建；wind-down 预算与残余任务清单；child 模式保留作故障隔离/回退；桌面 E2E 崩溃注入场景 |
| 免序列化分叉                                           | 两条链路行为漂移                            | 双传输一致性子测试（全 scenario 双跑）；帧边界/flush/关闭语义由同一子 spec 定义                                    |
| 多平台                                                 | Windows 主力先行；macOS 签名/公证、Linux 包 | CI 三平台 + 阶段 3 打包矩阵（已有 6 目标与签名开关）                                                               |
| 上游漂移（3.15.1+）                                    | 差分基准过时                                | 沿用现有 drift 文档机制，每期开工前核对                                                                            |
| Node 删除后 oracle 失效                                | 等价性从此不可重验                          | 阶段 3 前完成 golden freeze；差分套件切到 goldens 模式全绿后才允许删 Node                                          |
| 实现质量债（panic 挂死路径、投递层零测试、游离 spawn） | 迁移期放大为事故                            | 先按『实现质量对齐工作包』P0→P1 清债，Host 迁移不建立在未修复的引擎上                                              |
| 许可证/归属                                            | 移植对标源码时的合规                        | 其 Cargo 声明 Apache-2.0；移植保留声明，核对来源许可                                                               |

## 7. 立即执行（第 0 期首批，本周）

1. [x] 修越界并让 `pnpm check:escode-cli-rust` 回绿（已完成，15+ 文件拆分）。
2. [x] 更新 `docs/rust-migration.md` 的过期表述（Rust 已默认，与 `rust-release-rollback.md` 对齐）。
3. rust-toolchain pin + clippy deny 清单已完成（CI 三平台同步 1.95.0）；`deny.toml` 待 cargo-deny 接入。
4. [x] App 级 perf harness 骨架（三配置 A/B、交错、双口径）——`scripts/bench-escode-app.mjs` 完成，`--self-test` PASS；**真实 A/B 基线已跑**（3 轮交错：App 工作集 A 1419.6 / B 1293.6 MiB，B 不劣于 A），报告 `docs/reports/rust-app-perf-baseline-2026-10-14.md`。
5. P0：catch_panic 已完成（红/绿验证）；投递层 `subscriptions.rs` 测试兜底、进程树三补丁、unwrap 热点清理——待做。
6. [x] 差分垂直切片 + 差异账本第一轮（2026-10-14）——`scripts/diff-escode-cli-rust-parity.mjs`（含 `-capture`/`-diff`/`-report`）与 `docs/reports/rust-difference-ledger-2026-10-14.md`；稳定复跑 3 次、负向验证通过（去掉一条基线指纹即判失败）、CI bench job 三平台接入（拦新差异）。
7. [x] M 系列真偏移收敛（2026-10-14）：模型请求字段名与 `tool_choice` 已与 TS/AI SDK 对齐（model_protocol.rs；parity 复跑两侧请求体一致，三处 TS 断言按 oracle 更新）。
8. [x] C-1/C-2 部分收敛（2026-10-14）：遥测事实与后续 ack 带 `memoryEnabled`、偏好请求提前到创建期发起；createSession ack 与请求位置登记为已批准。剩余 T-3（ARMS 埋点，随阶段 2 P2）与 T-4（投递层）见账本。

## 8. 阶段 1 执行记录（2026-10-14）

- 子 spec：`docs/specs/rust-u1-runtime-lib.md`（含设计、验收、基线与 u2 移交清单）；验收报告：`docs/reports/rust-u1-runtime-library-2026-10-14.md`。
- 新增/迁移：
  - `core-api/src/env.rs`（EnvSnapshot + InstanceContext，workspace 唯一 `std::env` 读取点）、`core-api/src/scope.rs`（Spawner）；
  - `app-server/src/link.rs`（Link/StdioLink/MemoryLink + EmbeddedIo 按行缓冲）、`src/embed.rs`（RuntimeOptions/Embedded）；
  - `host/src/file_stamp.rs`、`core/src/app/waiters.rs`；`src/runtime.rs` 移入 lib 目标供二进制与嵌入共用装配。
- 测试：嵌入生命周期 5 例（`tests/embed_memory_link.rs`：服务与 EOF 收口、双实例并发无串扰与存储隔离、双 spawn 所有权冲突 + 关闭后重 spawn、RuntimeOptions 的 env/cwd 驱动存储解析（databaseId 证明）、进程内控制往返延迟基线）、双传输一致 3 例（`tests/dual_transport.rs`）、Spawner 5 例、FileStamp 5 例、registry 闸门 4 例、waiters 2 例。
- 门禁：边界脚本新增「非测试代码 0 处进程 env 读取」规则（唯一豁免 core-api/src/env.rs）。
  - `cargo fmt --check`、`cargo clippy --workspace --all-targets`（-D warnings）、`cargo test --workspace`（433 通过 / 0 失败）；
  - `pnpm typecheck`、`pnpm lint`（0 error）、`pnpm architecture:check --changed` 全绿；
  - 差分套件 407 例中 `escode-cli-rust-legacy-session.test.ts` 为本机既存失败（已用阶段 0 基线二进制 bisect 排除本批引入，见验收报告 §5），其余全绿。
- **运行纪律**：
  1. 差分套件运行期间禁止并发 `cargo` 构建（会擦写同一个 `target/debug` 二进制，导致套件内用例假失败；第一轮实测教训）。
  2. **禁止 `git add -A`**：本仓库工作树可能同时承载其他工作流的未提交改动（本轮实测：usb agent tools 的 usb-plugin / usb-capture-helper / official_plugins.json 等），按路径精确暂存以避免把他的 WIP 卷进本线提交。
