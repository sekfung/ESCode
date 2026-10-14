# Rust 阶段 2：Host 骨架与分域迁移（rust-unified-host.md 子 spec）

> 状态：立项 · 2026-10-14
> 父计划：`rust-unified-host.md` §阶段 2；前置：阶段 1（子 spec `rust-u1-runtime-lib.md`，验收报告 `docs/reports/rust-u1-runtime-library-2026-10-14.md`）
> 参考实现：ZCode-rs `apps/zcode-cli-rust`（Host 与服务分层）、`apps/zcode-host`（若同构即对照）；对标仓库质量结论见 `docs/reports/rust-impl-quality-comparison-2026-10-14.md`

## 1. 目标与边界

把桌面 Host（现 `packages/desktop` 的 TS Host + 每 workspace 一个 Rust CLI 子进程）迁移为 `apps/escode-host`（Rust）：一个 App 一个 Host 进程，window→Host 经 attachment 接入，runtime 以 embedded 为默认（阶段 1 已具备 `embed::spawn`），child 模式保留作差分/回退/故障隔离。

本阶段分两个可交付批次：

- **u2a 骨架与通路**（先行，本文详述）：Cargo workspace 结构、基础设施（TaskScope 复用、有界通道、原子写、Gateway、control、capability 握手）、Host 服务契约清单与 `--check`、dev 开关与回退链，全部带单测/进程内测试。
- **u2b 分域迁移**（9 个服务域按优先序逐个交付，每域一份子 spec + 差分与 E2E 证据）：settings → workspace → sessions/tasks → providers/account → extensions → automation → remote + runtime-link。

### 1.1 Host 作用域决策（本阶段确认项）

父计划 §2 的边界变更：**现文档为 window-scoped Local Host，目标形态是一个 App 一个 Host（所有窗口经 attachment 接入）**。u2 确认采用目标形态，理由：

- 一份实例表/代次/订阅归属才能真正共享 runtime 实例与缓存（父计划阶段 4「跨实例共享缓存」的前提）；
- 无窗口调度（cron/offpeak）必须有独立于窗口的宿主（父计划 §2 状态所有者：Host 拥有调度）；
- 窗口关闭不应终止 runtime（attachment 断开 ≠ Host 停止）。

实施时（u2b 落地批次）同步更新 `AGENTS.md` 的进程与协议段落与相关 spec；在此之前 AGENTS.md 描述仍与当前代码一致，不改。

### 1.2 命名

现有 `apps/escode-cli-rust/crates/host` 是 CLI 的**配置/路径 adapter**（workspace 路径、env、凭证、进程组），与新 Host 概念重名。u2 内将其更名 `crates/support`（或 `crates/kit`），一次性改名 + 全仓引用更新；各子 spec 引用新名。

## 2. 工作区结构

对标三 workspace（Host 不拖 CLI 编译与依赖）：

```
apps/escode-host/
  Cargo.toml                 # 独立 workspace（phase 2 起点：仅本 workspace 的 crate）
  crates/
    host-core/               # 基础设施：TaskScope（复用 core-api::scope）、有界通道、原子写、进程回收
    host-gateway/            # 连接注册表、到达序 lane、出站队列、capability 握手、RPC 路由
    host-control/            # Main↔Host stdio 控制（日志与控制同一有序队列）
    host-services/           # 服务域聚合（u2b 起按域拆分：settings/workspace/sessions/...）
    host-proto/              # Host↔Renderer 协议类型（从 packages/shared 生成/校验，L1 契约）
```

共享基础：**直接复用阶段 1 的 `escode-cli-core-api`**（`env::EnvSnapshot`、`scope::Spawner`、`catch_panic`）而不是复制——`apps/escode-host` 以 path 依赖引用 `apps/escode-cli-rust/crates/core-api`。这是 U1 库化的直接红利，也是「基础 crate」优先序第 1 项的落地方式（复用，非重写）。

## 3. 基础设施（u2a 交付物与验收）

### 3.1 TaskScope

复用 `core_api::scope::Spawner`：唯一 spawn 入口、panic sink 上报、`running()` 残余清单、`drain(grace)` 宽限、实例 env 传播。Host 侧每个服务域各持一个 Spawner（父子 scope 语义：域结束时中止本域任务）。验收：Host 收口后各域 `running()` 为空。

### 3.2 有界通道（每队列写明溢出策略）

| 队列 | 容量 | 溢出策略 |
| --- | --- | --- |
| Gateway 入站（Renderer→Host RPC） | 256 条 | 触发背压：连接级 `saturated` 状态，禁止新请求登记，等队列腾空（与 runtime 投递层同语义） |
| Gateway 出站（Host→Renderer 帧） | 256 条 / 字节水位 8 MiB | 超字节水位丢弃可重建流（快照恢复），控制帧不丢（到达序 lane 保序） |
| runtime→Host 事件 | 128 条（与 Engine 现有事件通道一致） | 满时阻塞 runtime 侧（承接现有语义，不新增丢弃路径） |
| Host 日志（file_log 批量） | 1024 条 | 丢弃最低级别并计数（warn 起不丢） |

每个队列的容量与策略在代码常量处注释引用本表；`host-core` 提供 `bounded::channel(name, capacity, policy)` 统一构造与指标（队列深度、丢弃数）。

### 3.3 原子写与进程回收

- 原子写：rename 重试（EPERM/EBUSY/Win32 32/33）+ 父目录 fsync（质量报告 §5 给 Z 更好的落点；u2a 内实现并单测）。
- 进程回收：复用 `escode-cli-host` 的 process_group / process_tree 三补丁（阶段 0 已完成），Host 侧不再自建。

### 3.4 Gateway（连接注册表与到达序 lane）

```
Renderer ──WS──► accept ──► 登记（同步，决定 lane 位置）──► 每连接 lane（单写者）
                                    │
                                    ├─ 请求 → RPC 路由（Host 服务 / runtime 会话）
                                    └─ 事件 → 订阅投递（topic 归属由 runtime 拥有）
```

关键规则（父计划 §2 与质量报告 §7）：

- **登记必须在请求到达时同步完成**：连接（或请求）进入 lane 的顺序由 accept/read 的到达序决定，不能依赖任务调度顺序——否则并发请求的应答顺序漂移。
- 每连接一个写者：出站帧按 lane 顺序写出，控制帧优先但不插队数据帧的内部顺序。
- capability 握手：连接建立即协商（客户端 capabilities ∩ Host 支持的 method 集），未实现成员立即回 `NotImplemented`（Renderer 不挂起，缺口显式可见）。

### 3.5 Main↔Host control

stdio 上单条有序队列：日志与控制消息共用一条写者（避免交错破坏协议）；控制消息 typed（`ControlMessage` enum：`WindowAttach/Detach`、`HostReady`、`Shutdown`、`LogBatch`），错误用结构化 code。

## 4. 服务契约清单（L1 门禁）

`host-proto` 导出 channel/method 清单（单一来源），`--check` 与各服务实现的存在性对账：

- 清单与源码不一致即失败（生成器 + `--check`，接入 `pnpm check:escode-host`）；
- 未实现成员在 Gateway 立即回 `NotImplemented`，并出现在契约清单的 `pending` 分组（缺口显式可见，不静默 404）；
- 契约清单是阶段 3 前 golden freeze 的输入之一。

## 5. 分域迁移（u2b 优先序与每域验收模板）

| 顺序 | 域 | 交付判定（每域统一） |
| --- | --- | --- |
| 3 | settings | 唯一写者；Node/Rust 双 Host 交替写读演练通过（同文件同格式） |
| 4 | workspace（git/terminal/watcher/媒体预览） | 桌面 E2E 交互热路径场景通过；无 Node 残留子进程 |
| 5 | sessions + tasks + tasks_index | Node Host vs Rust Host 场景双跑差分全绿；tasks-index DDL 字节冻结 + checksum |
| 6 | providers + account | 模型请求差分（含 Registry 快照与账号 overlay）全绿 |
| 7 | extensions（skills/plugins/hooks/mcp/commands/memory） | 文件系统视图与 TS 逐字段对照（含 hooks 信任锁） |
| 8 | automation（cron/offpeak/bots） | 无窗口调度场景：窗口全关仍在跑；调度事件顺序与 TS 一致 |
| 9 | remote + runtime-link 选择器 | `desktop-continuous` 与 `web-remote-replayable` 双链路语义 + 手机远控 attachment 复用场景 |

每域开工前按父计划子 spec 约定写 `docs/specs/rust-u2<序号>-<域>.md`。

## 6. dev 开关与回退

- 单一开关（env `ESCODE_RUST_HOST=1`，dev 默认关；阶段 3 翻转默认）同时决定 Host 实现；开关只影响装配，不影响任何协议语义。
- child 模式：`runtime-link = child` 时 Host 以 `app-server --stdio` 拉起 CLI 子进程（阶段 1 之前的形态），用于差分、回退与故障隔离。
- 回退矩阵与演练沿用 `rust-release-rollback.md`（阶段 3 扩展 installed 四平台）。

## 7. u2a 实施进度（2026-10-14）

已落地（工作区 `apps/escode-host`，可通过 `pnpm check:escode-host` / `pnpm test:escode-host` 验证）：

| 组件 | 内容 | 测试 |
| --- | --- | --- |
| `host-core` | `bounded`（命名队列 + 溢出策略枚举 `Backpressure`/`DropReconstructible`/`BlockProducer`/`DropLowestPriority` + 深度/丢弃指标）、`atomic_write`（临时文件 + fsync + rename 重试 EPERM/EBUSY/32/33 + 父目录 fsync，失败清理临时文件） | 背压等待 vs 丢弃计数、关闭语义；原子写替换/建父目录/失败清理 |
| `host-proto` | 契约清单（channel/method + `Implemented`/`Pending` 状态）与 `--check` 对账（重复项、未登记的实现、清单外登记三个方向） | 清单自洽、正/负向对账 |
| `host-gateway` | 连接注册表（`register` 同步取号 → lane 序 = 到达序）、每连接出站 lane、capability 握手（∩ 支持面）、未覆盖 method 回 `NotImplemented` | 到达序、能力求交、路由缺口、注销 |
| `host-control` | typed `ControlMessage`（WindowAttach/Detach、HostReady、Shutdown、LogBatch）、控制与日志共用单队列（控制必达、日志满即丢）、结构化 `ControlError` code（可判重试） | 单队列顺序、队满下控制等待而日志丢弃、错误码序列化 |
| `host-runtime` | runtime-link 选择器（`Embedded` 默认 / `Child` 子进程）、实例表（每 workspace 一实例、**槽位 single-flight**、**懒重启 generation**、收口宽限与残余清单）；**读写分离**（帧泵独占读端、写请求不被阻塞）；embedded 收口 await 实例任务以保证 owner 锁释放 | 双 workspace 独立实例与代次、退出后下次 get 代次 +1、并发 get 只启动一代、close/shutdown 残余为空、**child 链路与 embedded 同就绪帧**（CLI 二进制在场时运行） |
| `host-services`（第一域：settings） | settings 域：`<home>/.escode/v2/setting.json` 与 TS Host **同文件同格式**；读取短重试 3×300ms → 坏文件隔离 `.corrupt-*` → 回退默认值（TS `settingService` 同语义）；写入**顶层合并**（保留文件里其它键，含 TS 的 legacy 账户连接字段）、2 空格缩进、`host-core::atomic_write`；控制协议新增 `settingsRead`/`settingsWrite`/`settingsValue` | Rust 5 例（缺失/合并保留未知键/坏文件隔离/合法非对象不隔离/读 TS 形状文件）+ **双 Host 交替写读演练**（`packages/services/tests/escode-settings-host-interop.test.ts`：真实 TS 设置服务 ↔ 真实 `escode-host` 进程，Node 写→Rust 读、Rust 写→Node 读、3 轮交替、未知键保留、缩进一致） |
| 实例出站流（host-runtime） | 读端收敛到实例内的一次读循环：按 `id` 关联 `call` 的等待方，其余帧扇出给 `subscribe()` 的订阅者（每个 attachment 一个）；Host 因此能对 runtime 发**请求/应答**（域迁移的前提），不再由各 surface 各自读帧 | 实例表 5 例（含订阅式就绪帧）、RPC 面 `escode-session.listSessions` 端到端 |
| `initializeWorkspace`（workspace 预热） | TS `agentService.initialize` 承担 host 启动预热并返回协议常量，首屏不因 agent 缺失崩溃。Rust Host 同构：建/复用实例后**等运行时报到**（`startup/storageState` phase=ready，`Instance::wait_ready`），返回 `{available, workspaceKey, protocolName:"ESCode Protocol", protocolVersion:1, transportKind:"stdio"}`（嵌入与 child 都走 stdio 帧面）；实例起不来时仍回成功应答 + `available:false` | Rust 端到端（预热后实例在 `/api/server-info` 可见、协议常量正确、dataDir 不可用时 `available:false`）+ 真实客户端（`initializeWorkspace` 返回协议常量） |
| `escode-session` / `readWorkspacePresentation`（优先序 4 的 workspace 读面起点） | 展示面读取按 TS agent 层同序：先 `runtime/capabilities` 判执行能力位，再带 `includeExecutionCapabilities` 读 `workspace/readPresentation`；已登记差异：TS 读取前同步账号 provider 配置（属 providers/account 域，未迁移），运行时按 Main 提供的配置自行解析 | Rust 端到端（展示面含 mode/slashCommands/执行能力位）+ 真实客户端（`readWorkspacePresentation` 返回 mode 与命令目录） |
| `escode-session`（优先序 5 的读面） | TS Host 该域是对 Agent 协议的直接转发（`result` / `result.<field>` 原样返回），Rust Host 同构：服务级参数 → 运行时路由（`workspace` 引用按 TS `buildWorkspaceRef` 构造，可选字段缺省即省略——运行时解析严格拒绝 null）→ 建/复用该 workspace 实例 → 转发 `session/list`、`session/read`、`session/messages`；运行时错误按 `RuntimeError` 透传；写面（`closeSession` → `session/close` 含条件关闭语义、`setModel`/`setThoughtLevel`/`setMode` → 同名 legacy 方法，运行时把 legacy 翻译成 V4 命令走同一写路径）；未迁移方法显式缺口并注明原因（`createSession`/`resumeSession` 需 Host 侧特性门参数整形、`closeDeferredDraftSession`/`promoteDeferredDraftSession` 需 Host 侧 draft 注册表、`readSessionEvents` 运行时无 `session/events`、workspace/provider 握手类属各自域） | Rust 端到端（`listSessions` 空列表、`/api/server-info` 报告实例与能力清单、`readSession` 不存在的会话 → `Session unavailable`、`readSessionMessages` → `not active`、`createSession` → 显式缺口）+ **真实客户端**（`listSessions` 返回 `[]`、`readSession` 透传 `Session unavailable`、`closeSession` 返回条件关闭成立、`createSession` 显式缺口） |
| `host-rpc` | **Renderer 真实链路**：`@escode/rpc` 协议跑在 WebSocket 上（13 字节帧 + VSBuffer 序列化 + channel 分派），HTTP 契约对齐 `server-core/http.ts`（`/api/server-info`、`POST /api/rpc-host-capability`、`/ws`、`/ws/host` 带 `x-escode-rpc-host-capability` 一次性令牌、非回环 fail-closed）；`setting` channel 接设置域（`get`/`update`），未迁移 channel/method 回显式 `NotImplemented`；`ProxyChannel.toService` 的实参数组打包已按 TS 语义处理 | Rust 17 例（帧切分/超长拒绝、VQL 与 TS 字节序一致、请求头布局字节级夹具、取消无响应、settings 往返、缺口显式、capability 一次性与 401、非回环拒绝）+ **真实 `@escode/client` 集成**（`packages/desktop/test/rustHostRpc.test.ts`：`connectViaProtocol` 调 `settingService.get/update` 并校验落盘，未迁移 channel 收到 `NotImplemented`） |
| `host-surface` | Renderer 传输面（WebSocket）：`Hello{windowId,clientId,workspace,capabilities}` → `Ready{generation,capabilities}` 握手（`deny_unknown_fields`），capability 与 Host 支持面求交，未覆盖 method 回 `-32601 NotImplemented`；**帧面沿用既有 V4 App NDJSON**（Renderer 协议客户端可原样复用）；每连接一个写者、一个实例一个帧泵向全部 attachment 扇出、慢连接按 `try_send` 丢弃可重建流；`OptionsFactory` 由组合根注入（crate 内不读进程 env）；Host 进程经控制消息 `surfaceListen`/`surfaceListening` 开关并随 Shutdown 收口 | 3 例 WS 端到端（握手与路由/缺口显式、多连接共享同一实例并扇出、空能力只收错误）+ Host 进程级 1 例（`surfaceListen` → 真实 WS 连接 → `session/list` 往返 → 干净退出） |
| `escode-host`（进程入口） | 控制循环（stdio NDJSON `ControlMessage`：attach/detach/请求转发/关闭）、按实例帧泵向多窗口扇出、link 选择器（`ESCODE_HOST_RUNTIME_LINK=child`）、结构化 `Failed`/`Stopped` 回执、`WorkspaceDescriptor→RuntimeOptions`（业务事实来自 Main，不从 Host env 推断） | 端到端两例（真实子进程）：hostReady 握手 → attach 得代次 → `session/list` 经 WindowFrame 往返 → 第二窗口复用同代次 → detach 后原窗口继续服务 → shutdown 退出码 0（`Stopped{residual:[]}`）；未 attach 窗口的请求回结构化错误 |

契约清单已与源码对账：`host-proto` 的 `control`/`settings` 通道登记 `Implemented`，并由 `control_inventory_matches_the_protocol_enum` 漂移测试保证与 `ControlMessage` 的 kind 集合一致（新增消息不登记即失败）。

CI（2026-10-14 已接入 `rust-runtime-ci.yml`）：checks 作业加 `pnpm check:escode-host`（边界 + fmt + clippy）与 host 工作区缓存；test 作业（三平台）加 `pnpm test:escode-host`（cargo 单测 + 双 Host settings 交替写读演练）。

dev 开关（`ESCODE_RUST_HOST=1`，2026-10-14 落地）：Main 懒启动一个 App 级 Rust Host（`rustHostManager.ts`：single-flight 启动、失败抛错由调用方回退 Node Host、退出时收口），窗口/远控 attachment 经 `rustHostBridge.ts` 把 MessagePort 字节桥到 Rust Host 的 `/ws`（消费方协议不变：端口上仍是 `@escode/rpc`）；开关接入 `webRemoteControlSharedHostAttachments.attachLocalHost` 的本地分支（消费者只用 `attachmentId` 与 `port`，已核对）。测试：`packages/desktop/test/rustHostBridge.test.ts`（4 例：开关判真、启动+桥接+真实客户端读写与落盘、manager 门控与路径解析）。

未落地（u2a 余项）：**桌面窗口自身 host 的进程无关化**——`desktopHostProcess.ts:762` 的 `MessageChannelMain` 已把 `port1` 给 renderer、`port2` 给 Electron utility Host，切到 Rust Host 需 `windowHostProcessMap` / `broadcastHub` / `taskRealtimeBus` / `registerHostProcess` 等子系统接受"非 utility process 的 window host"（独立改造，不建议与协议面同批）；`Hello/Ready` 握手与分片预算的正式 spec 化（当前最小增量登记于本表）。

TaskScope 与进程回收按 §3.1/§3.3 复用 `escode-cli-core-api::scope` 与 `escode-cli-host`，本工作区不重复实现（已验证可跨 workspace path 依赖）。

## 8. u2a 验收（本阶段第一部分）

| 验收项 | 证据 |
| --- | --- |
| 三 workspace/结构就位，Host 骨架可编译 | `cargo check -p escode-host-*` 全绿；`pnpm check:escode-host` 含边界检查 |
| TaskScope 收口 | 每域 `running()` 为空；残余任务告警有测试 |
| 有界通道溢出策略 | 每队列策略单测（背压/丢弃可重建流/丢弃计数） |
| Gateway 到达序 | 并发请求登记顺序 = accept 顺序（确定性测试）；未实现方法回 `NotImplemented` |
| capability 握手 | 客户端能力与 Host method 集求交；不支持能力不登记 |
| control 有序 | 日志与控制同队列顺序测试；shutdown 语义（grace → 残余清单） |
| 契约清单 `--check` | 清单与实现漂移即失败（负向验证：故意删一个 method 判失败） |
| 双链路 | u2a 内先以 `runtime-link=embedded`（阶段 1 `embed::spawn`）+ 最小 settings 读取跑通；child 链路同场景等价 |

## 8. 风险

| 风险 | 对策 |
| --- | --- |
| Host 体量（14 万行量级） | 分域交付 + 每域差分/E2E 门槛；未达标不进入下一域 |
| 单进程爆炸半径 | per-instance panic 重建（阶段 0 catch_panic + 阶段 1 Spawner）；child 模式作故障隔离 |
| 双 Host 状态互操作 | settings/tasks-index 同文件同格式 + 跨进程文件锁 + DDL checksum（父计划 §2 边界与幂等） |
| 上游漂移 | 每域开工前核对 `rust-v3.15.1-drift.md` |
