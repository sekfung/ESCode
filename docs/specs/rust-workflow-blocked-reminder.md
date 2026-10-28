# Rust runtime：workflow blocked reminder

> 状态：已实现 · 2026-10-28
> 关联：[upstream-v3.16.2-sync.md](upstream-v3.16.2-sync.md)、[w2-port-rust-review.md](../upstream-sync/w2-port-rust-review.md)
> 上游参考：`apps/zcode-cli/packages/core/src/runtime-task/workflow-blocked-reminder.ts`、`runtime/methods/dynamic-workflow-blocked-reminder.ts`、`workflow-notification-copy.ts`

## 1. 行为

会话空闲、且有 workflow run 仍开着决定（升级问题或 waiting 留白）时，按空闲时长再提醒主代理：

- 第一次：空闲 15 分钟；
- 第二次：第一次提醒唤起的那一轮结束后，再空闲 30 分钟；
- 每个 run 最多两次；用户发话或该 run 的开集变化，重新给一对提醒；
- 提醒只汇总计数、每条决定的 id/名称/类型与最久等待，不重复决定原文；
- 提醒走既有 run 通知投递路径（后台结果轮），不新增队列、不新增协议方法。

不提醒的情况：会话在跑/有排队/有设置轮、`auto_drain=false`、run 没有开着的决定、次数用完。

## 2. 状态所有者

| 状态                             | owner                                                      |
| -------------------------------- | ---------------------------------------------------------- |
| 空闲段起点 `idle_since`          | Engine，按会话                                             |
| 每个 run 已提醒次数              | Engine，按会话 + runId                                     |
| 开集签名（qid / waiting siteId） | Engine，按会话 + runId                                     |
| 在飞 run 与开集事实              | `Session.workflow_runs` 投影（由 `workflowRuns` 通知维护） |
| 通知投递、去重、originMeta       | `workflow_settled` → `deliver_workflow_notices`            |

## 3. 实现

- 纯状态机：`crates/domain/src/workflow_blocked_reminder.rs`
  - `WorkflowBlockedClock::{session_busy, session_idle, user_spoke, open_set_changed, due}`；
  - `format_blocked_notification` 与 `blocked_meta`。
- 接线：`crates/core/src/app/workflow_blocked.rs`
  - Engine 的 1s refresh tick 调 `workflow_blocked_tick`；
  - `live_runs` 从 `session.workflow_runs["runs"]` 过滤 `status=running`，取 `pendingQuestions` 与 `state=waiting` 的 `holes`；
  - 到点后 `workflow_settled` 入同一通知队列，由 `deliver_workflow_notices` 在空闲时起后台结果轮。
- 重置：`input_admission.rs` 对真实用户 `sendText`（非 `subagent-coordinator`、非 history rerun）调用 `user_spoke()`；开集变化由 tick 的签名比较发现。

设计差异：

- 上游用 `setTimeout` 精确排程；Rust 用 1s tick 近似，误差 ≤1s，行为边界不变。
- 上游 `readOpen` 走 workflow 端口 `getTask`；Rust 直接读 `workflow_runs` 投影（与 `GetWorkflowRun` 同一份 pendingQuestions/holes 事实），避免每秒一次跨进程 RPC。投影缺席时视为没有在飞 run。

## 4. 验收

- 15 分钟到点发第 1 次；再空闲 30 分钟发第 2 次；之后不再发。
- 会话忙时不发；`auto_drain=false` 不发；run 没有开集不发。
- 用户发话后重新从第 1 次算；开集变化后空闲钟从变化时刻重起。
- 提醒进入 `workflow_notices`，空闲时作为 `backgroundResult` 轮投递；忙时留在队列。
- 文案包含 `<workflow-blocked>`、`<open-questions>`、`<open-holes>`、`<reminder>`；超 8 条时带 `shown` 与“未列出的去 GetWorkflowRun”。
- 单测：`cargo test -p escode-cli-domain workflow_blocked`、`cargo test -p escode-cli-core workflow_blocked`。

## 5. 非目标

- 不做跨进程恢复：进程退出后在飞 run 结算为 interrupted，resume 会重新产生通知。
- 不新增提醒次数配置；15/30 分钟是固定契约。
- 不改变 run 自身的 stalled 检测与 journal。
