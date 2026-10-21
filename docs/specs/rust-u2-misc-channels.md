# Rust 阶段 2：其余小通道（rust-unified-host.md 子 spec）

> 状态：持续追加 · 2026-10-17。体量小、彼此不相关的薄通道，每个通道一节：范围 / 事实源 / 登记差异 / 验收。
> 与 `rust-u2-user-config-services.md` 的分工：那边是「Host 读写家目录某约定位置」的纯 FS 服务，
> 这边是**运行时转发**与**网络读取**类的小通道。域再大一点就单独开子 spec。

## 1. `usage-stats`（1 个成员：`getAppUsageSnapshot`）

> **状态：已交付 · 2026-10-17**。`Instances::live_any()`（`host-runtime/src/lib.rs`）、`domains_usage_stats.rs`、
> 契约清单登记、单测 3 例 + 端到端 1 例（真 embedded 实例）全部落地；负向验证两次（改成错误方法名 →
> 端到端失败于运行时的 `Unsupported method`；去掉参数过滤 → 两条单测失败），均已恢复。

TS 真相源：`packages/services/src/usage-stats/usageStatsService.ts`（23 行）→
`packages/services/src/escode-agent/escodeAgentService.ts:4052` 的 `getAppUsageStats`。

### 1.1 语义：**不按 workspace 路由**

这是本通道与 `escode-session` / `escode-task` 转发面的关键区别：`usage` 表在**全局 session 库**，TS 取
`activeClientsByWorkspaceKey` 里**任意一个**已连接 client（代码注释：「复用任意已连接的 workspace client
即可拿到全应用范围数据」），**UI 也不传 workspace**（`packages/ui/src/hooks/useUsageStats.ts:44` 只传
`{range, timeZone}`）。因此 `forward::runtime_call` 那套「要求 `workspacePath`」的模板**不适用**。

行为逐条：

1. **不拉起运行时**（existing-only）：无活跃 client → 抛 `new Error("no_active_workspace")`。
   该错误在 UI 可见（`useUsageStats` 把它显示为错误态），不是内部错误。
2. **请求体整形**：只有 `{range, timeZone}` 两个键（TS 字面量 `{range: params.range, timeZone: params.timeZone}`），
   其余入参字段丢弃；`timeZone` 缺省时该键**省略**（运行时反序列化严格）。
3. **协议方法**：`v4/usage/stats`（`packages/shared/src/escode-protocol-v4/transport.ts:367`；旧的
   `usage/stats` 已 `@deprecated`、host 消费清零）。
4. **请求体不带 `workspace` 引用**（TS 这里没有 `buildWorkspaceRef`）。
5. **结果原样透传**（`AppUsageSnapshot`，`packages/shared/src/usage-stats.ts:90`），无 Host 侧再投影。

### 1.2 所有者

`Instances` 仍是实例表唯一所有者，**只读不写**。新增只读取用方法 `Instances::live_any()`：在 Host 内
「取任意一个活实例」是实例表自身的策略（选谁由表的迭代序决定），不应让每个消费方各自实现一遍
`identities()` + `live()` 的遍历 + 顺序选择。

### 1.3 登记差异

1. **选中哪个实例**：TS = `Map` 插入序第一个（**先连接**的 workspace）；Rust = 身份键**字典序最小**的活实例
   （`Instances.table` 是 `BTreeMap`）。**对结果不可见**——usage 表在全局库，任何 client 回同一份数据；
   差异仅在「多 workspace 同时活跃且其中一个故障」时可观察，而 TS 侧本身也不确定（取决于连接顺序）。
2. **`no_active_workspace` 的错误形状**：TS `new Error(...)` 下发为 `{name: "Error", message: "no_active_workspace"}`，
   Rust 同名同文案、**不带** `code` 字段（`RpcError` 默认无 extra）。
3. `v4UsageStatsResultSchema` 的 zod 解析不复刻（Rust Host 只驱动同版本内置 runtime，沿用既有口径，
   见 `rust-u2-agent-v4.md` §6）。
4. TS 的 `getAppUsageStats` 无日志；Rust 同（不新增诊断面）。

### 1.4 验收

- **单测**（`host-rpc/src/domains_usage_stats_tests.rs`）：无活实例 → `no_active_workspace`；参数整形
  （多余字段丢弃、`timeZone` 缺省省略、`range` 缺失时按 TS 同样下发）。
- **端到端**（`host-rpc/tests/rpc_usage_stats.rs`，真 embedded 实例 + 真 WebSocket）：
  1. 未拉起实例时 `getAppUsageSnapshot` → `no_active_workspace`（证明 existing-only，**不偷偷拉起运行时**）；
  2. 经 `escode-session.createSession` 拉起实例后同一调用 → 返回 `AppUsageSnapshot` 形状
     （`range` / `source: "agent-db"` / `summary.totalTokens` 在场）；
  3. 断言请求真的走 `v4/usage/stats`（而非缺口回显或本地短路）。
- **契约对账**：`inventory_services.rs` 的 `IMPLEMENTED` 加 `("usage-stats", "getAppUsageSnapshot")`
  （`tests/contract.rs` 双向对账；漏改即 CI 失败）。

## 2. `escode-agent` 旧面转发（已保存工作流 / Wiki / Skill catalog / Session debug）

> **状态：已补齐 · 2026-10-21**。这些成员是 `escode-agent` 旧通道里仍被 Renderer 直接调用、
> 但不在 V4 面方法表中的部分；事实源仍在 CLI runtime，Host 只做参数整形与路由。

| 成员                       | protocol 方法             | 路由                                                  | 请求体                                                   |
| -------------------------- | ------------------------- | ----------------------------------------------------- | -------------------------------------------------------- |
| `listSavedWorkflows`       | `workflows/list`          | 有 `workspacePath` 走该 workspace；否则走插件管理实例 | `{workspace, scope?}`                                    |
| `getSavedWorkflow`         | `workflows/get`           | 同上                                                  | `{workspace, name, scope?}`                              |
| `updateSavedWorkflowMeta`  | `workflows/updateMeta`    | 同上                                                  | `{workspace, name, meta, scope?}`                        |
| `deleteSavedWorkflow`      | `workflows/delete`        | 同上                                                  | `{workspace, name, scope?}`                              |
| `listSavedWorkflowRuns`    | `workflows/runs`          | 同上                                                  | `{workspace, name?, limit, scope?}`                      |
| `moveSavedWorkflow`        | `workflows/move`          | workspace 直通                                        | `{workspace, name}`                                      |
| `saveSavedWorkflowFromRun` | `workflows/save`          | workspace 直通                                        | `{workspace, runId, name, meta, scope?, overwrite?}`     |
| `findSavedWorkflowForRun`  | `workflows/forRun`        | workspace 直通                                        | `{workspace, runId, candidates?}`                        |
| `getWikiStatus`            | `v4/wiki/status`          | workspace 直通                                        | `{}`（`-32601` 折算 `{supported:false, enabled:false}`） |
| `getWikiGraph`             | `v4/wiki/graph`           | workspace 直通                                        | `query ?? {}`                                            |
| `getWikiEntry`             | `v4/wiki/entry`           | workspace 直通                                        | `{id}`                                                   |
| `manageWiki`               | `v4/wiki/manage`          | workspace 直通                                        | `request`                                                |
| `getSkillReferenceCatalog` | `skills/referenceCatalog` | workspace 直通                                        | `{workspace, sessionId?}`                                |
| `readSessionDebug`         | `session/debug`           | workspace 直通（existing-only）                       | `{sessionId}`                                            |

- **全局已保存工作流**：无 `workspacePath` 时按 TS `resolveGlobalSavedWorkflowCarrier` 的兜底，
  打到插件管理实例（固定内部 cwd）；全局档数据不依赖具体 workspace。
- **请求体整形**：`workspace` 引用按 `forward::workspace_ref_json` 构造；可选字段缺省时不写
  （runtime 的 strict schema 拒绝 `null`）。
- **错误**：runtime 的 JSON-RPC error 原样转成 `ESCodeProtocolClientError`（与既有转发一致）。

### 2.1 验收

- Rust 单测/RPC e2e：覆盖每个成员的路由与请求体；fake runtime 断言收到的方法与参数。
- 契约对账：`IMPLEMENTED` 加上述 14 个成员；`contract.rs` 双向对账通过。
- 负向验证：删掉 `readSessionDebug` 的 Implemented 登记，contract 必须失败。

## 3. `escode-agent` 插件旧面：复用 `plugin-management` 实现

> **状态：已补齐 · 2026-10-21**。TS `escodeAgentService` 的插件方法全部走
> `getPluginManagementClient()`，与 `plugin-management` channel 是同一个管理面实例。
> 因此 Host 把 `plugin-management` 的方法表与事件表原样挂到 `escode-agent`，不复制实现。

覆盖成员：`addPluginMarketplace`、`cancelPluginOperation`、`configurePlugin`、`describePlugin`、
`getPluginReferenceCatalog`、`getPluginsOverview`、`installPlugin`、`listPlugins`、
`onDynamicPluginOperationProgress`（事件）、`removePluginMarketplace`、`resetPluginConfig`、
`resolveSuggestedPluginReference`、`restoreBuiltinPlugin`、`setPluginEnabled`、
`uninstallPlugin`、`updatePlugin`、`updatePluginMarketplace`、`validatePlugin`。

- 两个 channel 共享同一个 `PluginManagement`（同一管理面实例与同一个进度 `Hub`），
  因此从 `escode-agent` 订阅的进度事件与 `plugin-management` 一致。
- 请求参数与超时完全沿用 `plugin-management`（`plugin_params::SPECS`）。

### 3.1 验收

- `domains_plugin_tests.rs` 断言两个 channel 的别名方法指向同一处理逻辑（同一 `operationId`
  的进度事件可跨 channel 收到）。
- 契约对账：上述 18 个成员加入 `IMPLEMENTED`；`contract.rs` 双向对账通过。

## 4. 运行时偏好、动态工作流选择与 CUA 观察

> **状态：已补齐 · 2026-10-21**。三者都是 Host 级状态/事件，不按 workspace 单次转发。

- `syncAppRuntimePreferences`：Host 保存最新快照（`modelIoFullRetentionEnabled` 强制布尔），
  对每个活实例发 `workspace/updateInteractionPreferences` 与
  `workspace/updateModelIoPreferences`；新实例在首个请求前追平（按 `(identity, generation)` 去重）。
- `syncDynamicWorkflowUserMode`：Host 保存用户选择，对每个活实例发
  `workspace/updateDynamicWorkflowPolicy`（`enabled` + `mode`）；新实例同样追平。
- `onDynamicCuaPermissionObservation`：Host 在实例首次使用时订阅其出站帧，把
  `v4/cua/permission-observation` 转成 Renderer 事件；远程 workspace 的观察事件不转发
  （与 TS 相同，由远端 Host 自己上报）。

## 5. Session debug

> **状态：已补齐 · 2026-10-21**。Host 转发 `session/debug`（existing-only），
> Rust runtime 侧新增只观察 `ModelStatus` 的快照器。

- 快照不落盘、不参与聊天投影；随会话记录回收。
- 已实现字段：`sessionId`、`rounds`（requestId/usage/hitRate/recordedAt）、
  `networkEntries`（started/completed/failed/retry_scheduled）、`cache` 汇总。
- 登记差异：runtime 的 `ModelStatus` 目前不带请求/响应 headers 与 `timeToFirstContentMs`，
  因此 `requestHeaders`/`responseHeaders` 为空、`generationDurationMs`/`tokensPerSecond` 为 `null`；
  移除条件：模型层把 headers 与首 token 时延写入 `ModelStatus`。

## 6. Host→Main 专属服务通道（2026-10-21）

- **协议**：控制流新增 `mainRequest { requestId, method, params }` 与
  `mainResponse { requestId, ok, result?, error? }`；Main 在 `rpcListen` 声明 `mainServices: true`
  表示会应答。Host 侧 `MainBroker` 按 `requestId` 配对，未知/重复应答丢弃。
- **当前方法**：`gen-ui.prepareSandbox`（Host 组装壳 HTML/初始状态，Main `registerFromHost`）。
- **未接入**：`cua-permission.getStatus/restartHelper` 需要把 CUA Helper 生命周期块从
  `createLocalServices` 提到 Main；`plugin-ui-bridge.prepareSandbox` 还需要 Rust runtime `mcp/ui*`。
- **错误**：Main 未声明 `mainServices` 或方法不支持时回显式 `NotImplemented`/`Error`，
  Host 不静默成功。
