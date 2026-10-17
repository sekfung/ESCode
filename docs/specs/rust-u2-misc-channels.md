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
