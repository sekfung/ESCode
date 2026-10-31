# Rust `control.availability` 投影对齐

2026-10-31。以 TS `apps/escode-cli/packages/bootstrap/src/escode-protocol-v4/projection-state.ts` 的
`computeAvailability` 为 oracle，对齐 V4 快照/补丁里的 `control.availability`。App 用这组布尔值决定
「压缩 / 分支 / 立即发送排队输入 / 暂停或恢复 Goal」按钮是否可点；reasonCode 同时是产品协议里的 guard id，
不能只对齐 `allowed`。

## 产品合同

| 动作            | 条件                                              | 结果                                                         |
| --------------- | ------------------------------------------------- | ------------------------------------------------------------ |
| `compact`       | 压缩进行中                                        | `{allowed:false, reasonCode:"compactOperationLock"}`         |
| `compact`       | `phase === "draft"`                               | `{allowed:false, reasonCode:"idleCannotCompact"}`            |
| `compact`       | 其他（running / completed / error）               | `{allowed:true}`                                             |
| `fork`          | 压缩进行中                                        | `{allowed:false, reasonCode:"compactOperationLock"}`         |
| `fork`          | `phase === "draft"`                               | `{allowed:false, reasonCode:"forkTargetNotStable"}`          |
| `fork`          | 其他                                              | `{allowed:true}`                                             |
| `sendQueuedNow` | 压缩进行中                                        | `{allowed:false, reasonCode:"compactOperationLock"}`         |
| `sendQueuedNow` | 有正在执行的轮次                                  | `{allowed:true}`                                             |
| `sendQueuedNow` | 其他（空闲）                                      | `{allowed:false, reasonCode:"sendQueuedNowRequiresRunning"}` |
| `pauseGoal`     | `goal.status ∈ {active, verifying, notSatisfied}` | `{allowed:true}`                                             |
| `pauseGoal`     | 无 goal                                           | `{allowed:false, reasonCode:"noGoalToPause"}`                |
| `pauseGoal`     | 其他 goal 状态                                    | `{allowed:false, reasonCode:"goalNotActive"}`                |
| `resumeGoal`    | `goal.status === "paused"`                        | `{allowed:true}`                                             |
| `resumeGoal`    | 无 goal                                           | `{allowed:false, reasonCode:"noGoalToResume"}`               |
| `resumeGoal`    | 其他 goal 状态                                    | `{allowed:false, reasonCode:"goalNotPaused"}`                |

`switchModelConfig` / `setFollowupMode` / `queueEdit` 恒为 `{allowed:true}`。

## 所有权与状态

- `control.availability` 由 domain `Session::availability()` 计算，`Session::patch()` 与 `snapshot()` 共用同一结果；
  core 只负责推进 `phase`、`run_id`、`compact_instructions`、`goal`，不各自另算可点性。
- 「压缩进行中」= `compact_instructions.is_some()` 且轮次仍在执行。`compact_instructions` 在压缩轮收口
  （成功 / 失败 / 中断）时必须清空；否则下一次普通轮次会把 `activeTurnKind` 误报为 `compact`，冷恢复后
  还会一直锁住 fork/compact。
- reasonCode 分两层：availability 用产品态 id（`forkTargetNotStable`、`sendQueuedNowRequiresRunning`、
  `idleCannotCompact`、`compactOperationLock`），命令拒绝用 guard id（`guard.forkTargetNotStable`、
  `guard.queuePromotionBusy` 等）。两者不能混用。
- `queued_now` 只影响命令级 `guard.queuePromotionBusy`，不参与 availability；Node 的 `compacting` 也不来自
  队列里的待执行 compact。

## 验收

- domain 单测覆盖 draft / running / compacting / completed 四种 phase、无 goal / active goal / paused goal /
  failed goal 的 reasonCode。
- `packages/services/tests/escode-cli-rust-session-projection.test.ts` 的 draft 快照用例在无平台 golden 的
  环境也校验整组 availability、`seq=0` 与快照默认键。
- `scripts/diff-escode-cli-rust-parity.mjs --golden` 的 session-baseline 中，draft 快照的
  `compact`/`fork`/`sendQueuedNow` 与 Node golden 一致；已知欠账基线随之收敛。
