# Rust 统一 Host：过往实现评审（2026-10-15）

> 方法：以源码与可运行证据为准，逐项核对 `rust-unified-host.md` / 交接手册 / 子 spec 中「已完成」的结论；
> 文档勾选项不作为事实。每条给出证据（file:line 或命令）、影响、处置。
> 关联：`docs/specs/rust-u2-agent-v4.md`（本轮立项的修复与新增面）。

## 0. 基线（接手时实测）

| 项 | 结果 |
| --- | --- |
| `node scripts/check-workspace-freshness.mjs` | 基线新鲜（dev 与 origin/dev 同步，ahead main 78） |
| `apps/escode-host` `cargo test --workspace` | 全绿（含上一会话未提交的 createSession WIP） |
| `pnpm check:escode-host` | **红**：`host-rpc/tests/rpc_session.rs` 超 400 行（WIP 引入） |
| `apps/escode-cli-rust` `cargo test --workspace` | 451 通过 / 0 失败 |
| P0#1 / P0#2 关键用例 | `a_panicking_run_still_reaches_a_terminal_phase`、`subscription_delivery_snapshot_pause_resume_close_and_replace` 存在且通过 |

## 1. 发现（按严重度）

| # | 严重度 | 发现 | 证据 | 处置 |
| --- | --- | --- | --- | --- |
| A9 | **高（安全）** | Main 把桌面 Renderer 桥到 Rust Host 的 **`/ws`（免鉴权）**；Host 在回环端口上暴露可驱动 agent/工具的完整服务面。本机任意进程，或浏览器页面（跨源 WebSocket 不受同源限制、DNS rebinding）在探测到端口后即可接入。capability 签发 `POST /api/rpc-host-capability` 同样免鉴权，令牌为「纳秒‖pid」可预测（A4）。TS 桌面窗口 Host 只走 MessagePort，没有网络面。 | `packages/desktop/src/main/rustHostBridge.ts:108`、`host-rpc/src/lib.rs`（`/ws` 分支不校验）、`Capabilities::issue` | 修复：拒绝带 `Origin` 的升级；全部 WS 路径要求一次性 capability；签发需要 Host 启动时经控制通道交给 Main 的进程密钥；令牌改 CSPRNG |
| A10 | **高（功能）** | 对话主链路未迁移：Renderer 的 V4 conversation/sessions-index/命令/附件全部经 `escode-agent` channel，Rust Host 无该 channel，也无事件面（`EventListen` 回 NotImplemented）。dev 开关下打开会话无法流式对话。交接手册与契约清单均未列出该 channel。 | `packages/ui/src/v4/agentConversationTransport.ts`、`host-rpc/src/session.rs`（`REQUEST_EVENT_LISTEN` 分支） | 新子 spec `rust-u2-agent-v4.md` |
| A5 | **高（功能）** | runtime→Host **反向请求无人应答**：帧泵把带 `method`+`id` 的请求当普通帧扇出。runtime 等待 `session/requestRuntimePreferences` 回包（记忆/Shell 偏好），无人应答时要等满 runtime 自身的超时才按缺省继续——实测每个新会话首轮**停滞约 30 s**（`rpc_agent_v4.rs` 负向验证：恢复旧扇出行为后回合 30.2 s，修复后 0.2 s；初稿写成「永久挂起」，经实测更正）。此前没有任何 Rust Host 用例跑过一轮真实回合。 | `host-runtime/src/lib.rs`（旧 `start_reader`）、CLI `shell_preferences.rs:65-139` | 已修：`HostResponder` + TS 同文 `-32601` 兜底 + 本地偏好应答（见 §2） |
| A3 | 高（功能） | RPC 会话**串行**处理请求（读循环内 `respond(..).await`）：一个慢请求（30 s 的 `session/create`、冷恢复）阻塞同窗口全部请求；`PromiseCancel` 不可能生效。TS `ChannelServer` 每请求独立 Promise。 | `host-rpc/src/lib.rs` `session()` | 修复中（并发 + 取消 + 事件） |
| A1 | 中 | 实例退出时**在飞请求不失败**：读循环结束但等待方未释放，每个 `call` 挂满 30 s 超时。 | 旧 `start_reader` 退出路径 | 已修 + 负向验证（去掉修复后用例等满 20 s 失败） |
| A2 | 中 | 订阅者扇出 `try_send` **满即静默丢帧**；已关闭订阅者从不摘除（每次 subscribe 泄漏一个 sender）。与 `rust-u2-host-skeleton.md` §3.2 队列表「满时阻塞 runtime 侧，不新增丢弃路径」相矛盾。 | 旧 `start_reader`、skeleton spec §3.2 | 已修：满即摘除并置 `lagged`（可观测，不静默）；spec 队列表同步 |
| A8 | 中 | child 链路 `stderr` piped 却无人读取：子进程日志写满管道缓冲后阻塞，实例停摆。 | `host-runtime/src/link.rs` `Link::Child` | 已修：逐行转发到 Host stderr |
| A11 | 中 | `escode-session` 「写/展示面完成」不成立：`setModel` 只转发（TS：非草稿时把快照与 `modelOverride` 写回索引并广播；草稿不写），`closeSession` 不清草稿，`readSession`/`setThoughtLevel`/`setMode` 缺 `withApiRetryRuntime` 的无状态部分（终态快照补 `runtime.apiRetry: null`）。均未登记差异。 | `escodeSessionService.ts:380-470` vs `host-rpc/src/domains.rs` | 修复（随 WIP 收尾） |
| A12 | 中 | WIP `createSession`/`resumeSession`：① 灰度 flag（offPeak/dynamicWorkflow）**从 Renderer 入参读取**，TS 由 Host 侧 client-config 判定、从不信任入参；② 不传 `sessionTraceId`（TS 在信封 `trace.traceId` 上带）；③ `resumeSession` 缺 thoughtLevel 重放（`canReplayThoughtLevelFromResumeSnapshot` + `setThoughtLevel`）；④ spec 写的端到端文件名（`rpc_task_snapshot.rs`）与实际（`rpc_session_create.rs`）不符。 | `session_params.rs`、`domains_session_write.rs`、`escodeSessionService.ts:221-380` | 修复（随 WIP 收尾） |
| A13 | 中 | 契约清单只手工登记 9 个 channel；TS 服务面实际 51 个 channel / 619 个成员。「清单与源码一致」只覆盖被登记的子集，未登记 channel（含对话主链路 `escode-agent`）的缺口不可见；且手工条目有**拼错的方法名** `file.workspaceFileListLength`（TS 实名 `listWorkspaceFilesLength`，交接手册照抄了错名），另漏登 3 个 `file` 成员。 | `host-proto/src/inventory*.rs` vs TS descriptor | 已修：生成器从 TS 源码解析全集 + `--check`；手工只登记已实现成员 |
| A15 | 中 | **模型请求读进程 env 代理**（已出厂的 Rust CLI 同样受影响）：`crates/net` 对 `Purpose::Model` 在未解析出代理时不调 `no_proxy()`，reqwest 便按进程 `HTTP_PROXY`/`HTTPS_PROXY` 代理；TS 未解析出代理即直连全局 `fetch`（Node 不读这些变量）。嵌入 Host 时读到的是 Host 进程 env 而非 Main 交给实例的快照，与阶段 1「显式环境」结论矛盾。实测：本机设了代理时，连回环 mock 模型的请求被送去局域网代理，回合一直重试 `fault.provider.serverError`。原单测注释自承「行为差异由 reqwest 内部代理发现决定，这里只保证入口可用」——即没有行为断言。 | `apps/escode-cli-rust/crates/net/src/lib.rs` `apply`/`apply_mcp` | 已修 + 行为回归用例 `crates/net/tests/model_direct.rs`（负向验证） |
| A6 | 低 | Host 仍用裸 `tokio::spawn`（skeleton spec §3.1 说「每个服务域各持一个 Spawner」）。 | `host-rpc/src/lib.rs`、`host-runtime` | 登记，随 TaskScope 收拢 |
| P0#1 | 低 | P0#1 的「`Panicked` 与 `StorageCommitFailure` 同等视为此后不得提交」未实现：run panic 只转为一次失败回合。用例只覆盖「终态必达」。 | `core/src/app/run.rs:200`、`engine_requests.rs:262` | 登记为 P0#1 残项 |

## 2. 修复记录

| 提交 | 覆盖 | 验证 |
| --- | --- | --- |
| `fd8a8668` fix(host-runtime) | A1、A2、A5（实例层）、A8 | `host-runtime/tests/host_io.rs` 5 例；A1 负向验证（去掉修复后等满 20 s 失败） |
| 本批 feat(host-rpc) 连接层 + 访问控制 + session 写面 | A3、A4、A9、A11、A12；A5 Host 侧本地偏好应答；skeleton spec 队列表与实现对齐 | `rpc_connection.rs` 3 例（负向：去掉 abort 后 2 例失败）、`rpc_surface.rs` 访问控制、`rpc_session_write.rs`、`session_params` 9 例、`index_sync` 4 例、`runtime_requests` 3 例；`rustHostRpc.test.ts` + `rustHostBridge.test.ts` 6/6（真实客户端走 capability + `/ws/host`）；`pnpm typecheck` 0 错、`pnpm lint` 0 错（89 条既有 warning） |

| 本批 feat(host-rpc) escode-agent V4 面 + 清单全量化 + net 直连 | A10、A13、A15 | `rpc_agent_v4.rs`（一轮真实对话）、`scope_routing` 3 例、`rustHostAgentV4.test.ts`、`model_direct.rs`；负向验证 3 组（暂存、反向请求、代理） |

附带修正：
- 早期帧：帧泵改为实例登记即启动后，首个订阅者之前的帧（`startup/storageState` 等）会丢——`instances.rs` 的既有用例
  `two_workspaces_get_independent_instances` 因此超时，补了有界早期缓冲（首个订阅者先收、溢出即 lagged）。
- HTTP 请求头读取加 10 s 总时限（半截请求头曾让连接任务每 2 ms 无限轮询）。
- `js_trim` 在 `task_projection/message_parts.rs` 与 `policy.rs` 各有一份实现（低：重复实现，待合并）。
