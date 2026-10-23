# Rust Host 阶段 2：automation 域（rust-unified-host.md 子 spec）

> 状态：M3 部分实现 · 2026-10-19
> TS oracle：`packages/services/src/session/automationService.ts`、`automationRepo.ts`、
> `automationCron.ts`；共享类型 `packages/shared/src/automation-types.ts`。
> UI：`packages/ui/src/store/automationManagementStore.ts`（经 `escode-agent` channel）。

## 1. 范围与状态

| 成员                                         | 状态                  | 说明                                                                                       |
| -------------------------------------------- | --------------------- | ------------------------------------------------------------------------------------------ |
| `listAutomations` / `listAllAutomations`     | Implemented           | `automations` 按 workspaceKey 过滤 / 全量，`created_at DESC`                               |
| `deleteAutomation`                           | Implemented           | 按 workspaceKey 归属删除                                                                   |
| `setAutomationEnabled`                       | Implemented（本地轴） | 更新 `enabled` + `lifecycle_status`（active/paused）                                       |
| `listAutomationRuns` / `deleteAutomationRun` | Implemented           | `automation_runs` 读取/删除，按 workspaceKey 过滤                                          |
| `createAutomation`                           | Implemented           | cron/scheduleRule 校验、相对延迟、创建上限 20、首次 `next_run_at`                          |
| `updateAutomation`                           | Implemented（子集）   | cron 重算、生命周期重算、scheduleRule 校验；见 §4 差异                                     |
| `restartAutomation`                          | Implemented           | 仅 `failed` 任务可重排；按 cron 重算 `next_run_at`                                         |
| `runAutomationNow`                           | Implemented           | manual run claim +（targetTaskId ? resume : createTask）+ 参数应用 + sendPrompt + 终态回写 |

## 2. 状态所有者与持久化

- 唯一事实：`<dataBaseDir>/.escode/v2/tasks-index.sqlite` 的 `automations` 与 `automation_runs`
  （与 TS 同库同表、同列）。
- Host 是管理面写者；`claimDue` / `markDispatched` / `markDispatchFailed` 在 repo，
  常驻 scheduler 在 Host 组合根启动（`automation_scheduler.rs`，20 s tick、misfire 跳过）。
  窗口全关后仍由 Host 进程派发，Main 不持有调度状态（H5.3 已落地）。
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

| #   | 差异                                         | 原因                                 | 移除条件                   |
| --- | -------------------------------------------- | ------------------------------------ | -------------------------- |
| 1   | 读取时 `mode` 只保留合法枚举，非法值字段缺省 | 与 TS `normalizeAutomationMode` 同判 | 不需要移除                 |
| 2   | 列表不派生模型可用性诊断                     | 属派发/Registry 域                   | 迁移调度与 Registry 后补齐 |

## 5. 调度所有者（H5.3）

- Host 拥有无窗口 scheduler：窗口全关后仍按 `enabled,next_run_at` 认领并派发。
- Main 只提交 `dataBaseDir`、workspace 描述与凭证；不承载 task/session 调度状态。
- 与 Rust CLI 内 cron 工具（`apps/escode-cli-rust/crates/domain/src/cron.rs`）分工：
  CLI 只处理单轮 `CronCreate/Update/Delete/List` 与 Host 端口调用；Host 负责持久化与调度。
- 已落地：`create → INSERT active → next_run_at`、`runNow → manual claim → createTask → sendPrompt → dispatched`、
  `claimDue → run claimed → dispatch → dispatched/failed → next_run_at/终态` 的常驻循环
  （misfire 跳过、transient 退避）。
- turn 终态：派发前订阅 `onDynamicTaskTerminalOutcome`，按 `inputId == runId` 过滤后回写
  `automation_runs.outcome`（succeeded/failed + error）。
- 绑定会话：`targetTaskId` 先 `resumeTask`，再应用保存的模型/思考档位/模式，最后以 runId 作为
  traceId 发 prompt；不再新建任务。

## 6. 验收

- Rust 单测/RPC e2e：`rpc_automation.rs` 覆盖 6 个成员 + create/update 往返；
  `automation::cron` 单测覆盖 cron/分钟锚点/一次性过期拒绝；
  `automation_dispatch::tests` 覆盖 run 终态按 runId 过滤与错误透传。
- 真实客户端/TS 差分：`packages/desktop/test/rustHostAutomationParity.test.ts`（2026-10-23）——同一
  tasks-index 库上，TS `AutomationService` 与真实 `@escode/client` 连 Rust Host 交替创建/暂停/
  改名/删除，双方 `list` 逐字段全等；Rust 生成的行 TS 可原样读回。
- 负向验证：删除 `escode-agent.listAutomations` 的 Implemented 登记，contract 必须失败。
