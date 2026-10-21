# Rust Host 阶段 2：automation 域（rust-unified-host.md 子 spec）

> 状态：M3 部分实现 · 2026-10-19
> TS oracle：`packages/services/src/session/automationService.ts`、`automationRepo.ts`、
> `automationCron.ts`；共享类型 `packages/shared/src/automation-types.ts`。
> UI：`packages/ui/src/store/automationManagementStore.ts`（经 `escode-agent` channel）。

## 1. 范围与状态

| 成员                                         | 状态                  | 说明                                                               |
| -------------------------------------------- | --------------------- | ------------------------------------------------------------------ |
| `listAutomations` / `listAllAutomations`     | Implemented           | `automations` 按 workspaceKey 过滤 / 全量，`created_at DESC`       |
| `deleteAutomation`                           | Implemented           | 按 workspaceKey 归属删除                                           |
| `setAutomationEnabled`                       | Implemented（本地轴） | 更新 `enabled` + `lifecycle_status`（active/paused）               |
| `listAutomationRuns` / `deleteAutomationRun` | Implemented           | `automation_runs` 读取/删除，按 workspaceKey 过滤                  |
| `createAutomation`                           | **Pending**           | 需要 cron 解析、`next_run_at` 计算、创建上限与 scheduleRule 归一化 |
| `updateAutomation`                           | **Pending**           | 需要 cron 重算、生命周期重算与 scheduleRule 校验                   |
| `restartAutomation`                          | **Pending**           | 需要 cron 重算 `next_run_at`                                       |
| `runAutomationNow`                           | **Pending**           | 需要 run claim、dispatcher 与目标 task 投递                        |

## 2. 状态所有者与持久化

- 唯一事实：`<dataBaseDir>/.escode/v2/tasks-index.sqlite` 的 `automations` 与 `automation_runs`
  （与 TS 同库同表、同列）。
- Host 是管理面写者；调度器（未迁移）负责 `claimDue`、`dispatch_status`、`retry_at` 与 run 结算。
- `model_selection` / `schedule_rule` 以 JSON 文本存储；读取时非法 JSON 按缺省字段处理（与 TS 读取兼容）。

## 3. 接口

`escode-agent` channel：

```ts
listAutomations({workspacePath, workspaceIdentity?}): ESCodeAutomation[]
listAllAutomations(): ESCodeAutomation[]
deleteAutomation({automationId, workspacePath, workspaceIdentity?}): void
setAutomationEnabled({automationId, enabled, workspacePath, workspaceIdentity?}): void
listAutomationRuns({automationId, workspacePath, workspaceIdentity?}): ESCodeAutomationRun[]
deleteAutomationRun({runId, workspacePath, workspaceIdentity?}): void
```

错误：SQLite/IO 失败回 `Error`；未实现成员由 Gateway 回显式 `NotImplemented`。

## 4. 已登记差异

| #   | 差异                                         | 原因                                   | 移除条件                   |
| --- | -------------------------------------------- | -------------------------------------- | -------------------------- |
| 1   | `setAutomationEnabled` 不重算 `next_run_at`  | Host 未接 cron 计算；调度器未迁移      | 迁移 cron 计算后补齐       |
| 2   | 读取时 `mode` 只保留合法枚举，非法值字段缺省 | 与 TS `normalizeAutomationMode` 同判   | 不需要移除                 |
| 3   | `create/update/restart/runNow` 未实现        | cron/生命周期/派发器依赖大，需单独切片 | 实施 §5 后补齐             |
| 4   | 列表不派生模型可用性诊断                     | 属派发/Registry 域                     | 迁移调度与 Registry 后补齐 |

## 5. 调度所有者（H5.3）

- Host 拥有无窗口 scheduler：窗口全关后仍按 `enabled,next_run_at` 认领并派发。
- Main 只提交 `dataBaseDir`、workspace 描述与凭证；不承载 task/session 调度状态。
- 与 Rust CLI 内 cron 工具（`apps/escode-cli-rust/crates/domain/src/cron.rs`）分工：
  CLI 只处理单轮 `CronCreate/Update/Delete/List` 与 Host 端口调用；Host 负责持久化与调度。
- 待实现事件顺序：
  `create → INSERT active → next_run_at → claimDue → run claimed → dispatch createTask/resume → dispatched → outcome → next_run_at/终态`。

## 6. 验收

- Rust 单测/RPC e2e：`rpc_automation.rs` 覆盖 6 个成员 + `createAutomation` 显式缺口。
- 真实客户端/TS 差分：待 create/update/runNow 迁移后补。
- 负向验证：删除 `escode-agent.listAutomations` 的 Implemented 登记，contract 必须失败。
