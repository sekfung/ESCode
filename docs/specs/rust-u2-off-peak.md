# Rust Host 阶段 2：off-peak-task 域（rust-unified-host.md 子 spec）

> 状态：M3 部分实现 · 2026-10-19
> TS oracle：`packages/services/src/session/offPeakTaskService.ts`、`offPeakTaskRepo.ts`、
> `offPeakServerClient.ts`；共享类型 `packages/shared/src/off-peak-types.ts`。
> UI：`packages/ui/src/store/offPeakTaskStore.ts:179`、`hooks/useOffPeakTaskNotifications.ts:37`。

## 1. 范围与状态

| 成员                                          | 状态                  | 说明                                                                                              |
| --------------------------------------------- | --------------------- | ------------------------------------------------------------------------------------------------- |
| `list` / `get`                                | Implemented           | tasks-index.sqlite 读取；list 联查 `tasks.title` 作为 `sessionTitle`                              |
| `updateTask`                                  | Implemented           | 仅 queued/paused 可编辑 title/prompt/permissionMode/modelSelection                                |
| `pauseTask` / `continueTask`                  | Implemented（本地轴） | queued↔paused 状态迁移；不做服务端票状态查询/重取号                                               |
| `cancelTask` / `deleteTask` / `deleteHistory` | Implemented（本地轴） | 终态不可逆出；非终态 delete 先 cancel；History 只写可见性标记                                     |
| `createTask`                                  | **Pending**           | 需要 POST /ticket 取号、账号凭证与 ModelSelection Registry 校验；Host 未接 Off-Peak server client |
| `getCodingPlanSupport`                        | **Pending**           | 需要账号/selected connection 的脱敏快照                                                           |
| `getTakeNumberAvailability`                   | **Pending**           | 需要服务端额度接口与凭证                                                                          |

## 2. 状态所有者与持久化

- 唯一事实：`<dataBaseDir>/.escode/v2/tasks-index.sqlite` 的 `off_peak_tasks` 表（与 TS 同库同表）。
- Host 是管理面写者；调度器（未迁移）将写 `schedulable/queue_position/next_poll_at/claim_running`。
- `model_selection` 存 JSON；读取时解析失败/缺失：
  - 非终态：`schedulable=false` + `modelSelectionIssue:{code:"repair-required", legacyModelId?, legacyReasoningLevel?}`；
  - 终态：不派生 issue。

## 3. 接口

```ts
list(): ESCodeOffPeakTask[]
get(id): ESCodeOffPeakTask | null
updateTask(id, {title?, prompt?, permissionMode?, modelSelection?}): ESCodeOffPeakTask | null
pauseTask(id): ESCodeOffPeakTask | null
continueTask(id): ESCodeOffPeakTask | null
cancelTask(id): ESCodeOffPeakTask | null
deleteTask(id): void
deleteHistory(id): ESCodeOffPeakTask | null
```

`updateTask` 经 `ProxyChannel` 是两个实参（id + params），Host 按数组解包。

错误：SQLite/IO 失败回 `Error`，message 保留底层原因；未实现成员由 Gateway 回显式 `NotImplemented`。

## 4. 已登记差异

| #   | 差异                                                                               | 原因                                                         | 移除条件                                              |
| --- | ---------------------------------------------------------------------------------- | ------------------------------------------------------------ | ----------------------------------------------------- |
| 1   | `continueTask` 只做 paused→queued，不查询票状态、不重取号                          | Host 未接 Off-Peak server client；重取号会消耗额度，不能伪造 | 迁移 server client 与凭证解析后补齐                   |
| 2   | `cancelTask` 不发送终态核销（settle），也不中止 running loop                       | 核销 outbox 与运行循环属调度器/运行时                        | 迁移调度与 run 控制后补齐                             |
| 3   | `updateTask` 不调用 Registry 重新校验 ModelSelection                               | Host 未持有 provider Registry 视图                           | 迁移 model selection 校验或由 Renderer 提供已验证选择 |
| 4   | `list/get` 的 `modelSelectionIssue` 只按“字段是否可解析”派生，不检查当前账号可用性 | 同上                                                         | Registry 可用后补齐                                   |
| 5   | 调度（claim/dispatch/poll/settle）完全未迁移                                       | H5.3/M5 范围；需要 Host 侧无窗口调度所有者                   | 实施 H5.3                                             |

## 5. 调度所有者（H5.3 前置约定）

- Off-Peak 的表与状态机归 Host；**调度循环必须由 Host 拥有**，不能挂在窗口或 Renderer 上。
- Main 只负责通过 `rpcListen.dataBaseDir` 提供 DB 根与凭证/Provider 配置；窗口关闭不得停止轮询。
- 与 Rust CLI 内 `off_peak` 工具（`apps/escode-cli-rust/crates/core/src/app/off_peak.rs`）的分工：
  CLI 只执行单次 idle turn；取号、排队、`schedulable`、终态核销由 Host 调度器负责。
- 事件顺序（待实现）：
  `create → POST /ticket → INSERT queued → poll → schedulable=1 → claim → dispatch createTask/resume → running → terminal → settle outbox`。

## 6. 验收

- Rust 单测：list/get 投影、selection issue、pause/resume、终态守卫、History、delete 级联。
- RPC e2e：`rpc_off_peak.rs` 覆盖 8 个成员 + `createTask` 显式缺口。
- 真实客户端/TS 差分：待 `createTask`/server client 迁移后补 L4。
- 负向验证：删除 `off-peak-task.list` 的 Implemented 登记，contract 必须失败。
