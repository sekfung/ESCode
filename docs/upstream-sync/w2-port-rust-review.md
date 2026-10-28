# W2 port-rust 行为核对（2026-10-28）

> 范围：`docs/upstream-sync/v3.16.2-manifest.tsv` 中 W2 的 91 个 `port-rust` 文件。
> 结论口径：`done` = 本轮已实现；`covered` = Rust 已有等价行为（给出证据）；`not-applicable` = Node 内部装配/oracle，不构成 Rust runtime 行为；`partial` / `gap` = 仍需跟进。

## 1. ToolSearch / DeferredToolCall（11 个文件）— done

`contracts/src/tools/{tool-search,deferred-tool-call}.ts`、`core/src/tool/search/*`、`core/src/tool/{deferred-call,handlers/tool-search}.ts`、`adapters/src/model/tool-search.ts`、`tests/.../tool-search-comparison.ts`、`agent/tool-search-hydration.ts`。

证据：

- `crates/domain/src/tool_search_catalog.rs`：模式解析、目录、搜索、DeferredToolCall 解析、native 引用块。
- `crates/core/src/app/agent_loop_surface.rs`、`tool_dispatch.rs`：控制工具注入、浮出、发现集合。
- `crates/model/src/tool_search.rs`、`tool_reference.rs`、`model_media.rs`、`anthropic_body.rs`：native beta、`defer_loading`、`tool_reference` 投影与降级。
- 差分：`crates/domain/tests/tool_search_differential.rs`（12 cases，oracle 来自上游 `search.ts`）。

## 2. 会话存储 / 迁移（15 个文件）— covered / W3

`adapters/src/storage/session-store/*`、`bootstrap/src/app/session-store.ts`、`contracts/src/interfaces/session-store.port.ts`。

- Rust 持久化在 `crates/state/src/storage_*.rs`（index、read、write、history、listing）。
- 上游 schema/migration 的 **数据模型** 变更属于 W3（project-storage / tasksDatabase），本轮未合并；Rust schema 生成器也依赖 W3 的 Host 服务。
- 结论：现状读写路径 covered；新增迁移列为 W3 缺口，不在 W2 假装完成。

## 3. Session execution / residency（8 个文件）— covered / not-applicable

`bootstrap/src/session-execution/*`、`bootstrap/src/zcode-protocol/session-residency.ts`、`core/src/runtime/methods/residency.ts`、`bootstrap/src/subagents.ts`。

- Rust actor 的常驻/冷恢复：`crates/core/src/app/session_residency.rs`（`read_cold_session`、`trim_resident`）。
- 输入准入：`crates/core/src/app/input_admission.rs`（`admit_input`、`command_prompt`）。
- `residency-lease` 服务的是 Bootstrap detached runner 的 record 回收；Rust 没有该 pool/record 结构，actor 生命周期由 Engine 持有，**not-applicable**。
- `trackResidencyBlockingWork` 对应 Rust actor 的 in-flight 作业集合；本轮未新增计数器，现有 `running()/queue` 语义 covered。

## 4. V4 命令 / 投影（17 个文件）— covered，新增 reconcile 已补

`bootstrap/src/zcode-protocol-v4/*`（cold-session-resume、command-inbox、handlers/\*、input-admission、prompt-turn、publisher、product-projection、sessions-index-projection、transcript-hydration、v4-gateway 等）。

- Rust 路由：`crates/core/src/app/engine_requests.rs`；查询：`queries.rs`；投影：`event_projection*.rs`、`session_read.rs`；准入：`input_admission.rs`。
- 本轮补上 `v4/sessionsIndex/reconcile`：`queries.rs::sessions_index_reconcile`，按 `sessionIds` 读 `load_index`，缺席返回 `exists:false`，不激活 runtime。
- `command-inbox` 的串行 admission 已由 `input_admission.rs` 承担。
- `cold-session-resume` 由 `session_residency.rs::read_cold_session` 承担。

## 5. Compact（8 个文件）— covered

`core/src/compact/{manual,microcompact}.ts`、`core/src/runtime/{helpers/compact,methods/compact*}.ts`。

- `crates/domain/src/context.rs`（`microcompact`、summary 合并）、`crates/core/src/app/run.rs` / `maintenance.rs`（compact 触发与持久化）。
- 上游本轮的改动是拆分与提醒持久化辅助；Rust 已有等价 compact 路径，未发现新的可观察行为。

## 6. Tool result settlement（5 个文件）— done

`core/src/runtime/tool-result-settlement/*`、`core/src/runtime/methods/tool-result-settlement.ts`。

- Rust `agent_loop.rs` 在工具结果批次内按原顺序 push 到模型历史，媒体/`tool_reference` 走 part 数组投影；失败/拒绝路径已覆盖。
- `modelCallWrapper`：`tool_search_catalog::model_call_wrapper` 识别包装入口；`agent_loop` 与 `event_projection` 在模型历史/持久化工具消息上写 `_escode_model_call_wrapper: "DeferredToolCall"`。

## 7. Workflow blocked reminder（2 个文件）— done

`core/src/runtime-task/workflow-blocked-reminder.ts`、`core/src/runtime/methods/dynamic-workflow-blocked-reminder.ts`。

- 纯状态机：`crates/domain/src/workflow_blocked_reminder.rs`（15/30 分钟、两次、用户发话/开集变化重置、文案）。
- 接线：`crates/core/src/app/workflow_blocked.rs`（Engine 1s tick、读 `workflow_runs` 投影、走 `workflow_settled`/`deliver_workflow_notices`）。
- 设计差异：tick 近似 setTimeout（≤1s）；`readOpen` 用 `workflow_runs` 投影替代每秒跨进程 RPC。见 [rust-workflow-blocked-reminder.md](../specs/rust-workflow-blocked-reminder.md)。

## 8. Runtime reminder persistence（1 个文件）— covered

`core/src/runtime/methods/runtime-reminder-persistence.ts`。

- Rust 的提醒直接写 session rows（`turnHeader`/`userInput`/`workflow_notices`），随 `persist` 落库；正文与 originMeta 同批写入。
- 上游的 `runtimeMessage` part metadata 是 Node 历史 part 结构；Rust 行模型等价，**covered**。

## 9. Subagent（4 个文件）— covered

`core/src/runtime/methods/subagent.ts`、`core/src/subagent/{runner,explore-tools,persistent-memory}.ts`、`contracts/src/interfaces/subagent.port.ts`。

- Rust：`core/src/app/subagent_tools.rs`、`subagent_query.rs`、`subagent_completion.rs`、`domain/src/subagent*.rs`。
- Agent/SendMessage/TaskOutput/TaskStop、profile/tools/maxTurns、子会话与取消树已有实现与差分语料。

## 10. Model runner（6 个文件）— covered

`adapters/src/model/{runner,runner-options,runner-stream,runner-generate,runner-diagnostics}.ts`。

- Rust：`crates/model/src/provider*.rs`、`anthropic_body.rs`、`anthropic_stream.rs`、`model_failure.rs`、`model_stream.rs`。
- 本轮新增 native tool search beta 与 `defer_loading`；其余 runner 拆分是 Node 内部结构，Rust 对应路径已存在。

## 11. MCP pool / provider routing 契约（2 个文件）— covered / not-applicable

`adapters/src/mcp/pool.ts`：Rust `crates/tools/src/mcp_hub.rs`（连接复用、失效、回收）。
`contracts/src/interfaces/provider-endpoint-routing.port.ts`：类型契约；Rust 实现位于 `crates/model/src/config.rs` / `registry_rules.rs`，**not-applicable** 作为 TS 类型文件。

## 12. Node services / oracle（8 个文件）— not-applicable

`packages/services/src/zcode-agent/*`。

- 按 Q2，Node CLI/runtime/Host 只作 oracle；这些文件的改动不合并为产品代码。Rust 对应行为在 `crates/core` 与 `crates/app-server`。
- 需要在后续差分中覆盖，但不属于“把 Node 代码搬进 Rust”。
- **ToolSearch 差分**：本 fork 的 Node CLI 没有合并上游 ToolSearch（Q2：Node 只作 oracle）。差分直接跑上游 `search.ts` oracle：`packages/services/tests/tool-search-oracle.ts` + `escode-cli-rust-tool-search.test.ts` 启动 Rust `examples/tool_search_fixture`，对同一 catalog/query 实时比对（19 cases）；另有冻结语料 `tool_search_cases*.json`（12 + 17 cases）。这覆盖搜索算法，不覆盖 Node CLI 的进程/协议接线。

## 13. 剩余缺口汇总

| 缺口                                 | 归属  | 说明                          |
| ------------------------------------ | ----- | ----------------------------- |
| project-storage / tasksDatabase 迁移 | W3    | 数据模型与 Rust schema 生成器 |
| 其余 W3–W5                           | W3–W5 | 见 `upstream-v3.16.2-sync.md` |
