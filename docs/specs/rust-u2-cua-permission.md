# Rust 阶段 2：`cua-permission` 通道（Host → Main）

> 状态：立项 · 2026-10-22
> 父计划：`rust-unified-host.md` §阶段 2；边界依据：`rust-u2-host-scope-boundary.md` §2.1
> 相关：`rust-u2-host-skeleton.md` §3.5（Main↔Host control）、`rust-u2-host-resilience.md` §3

## 1. 目标与边界

契约清单里 `cua-permission` 的 `getStatus` / `restartHelper` 是 M3 最后两个 Renderer 可达缺口
（`ComputerUseSection`、`useCuaPermissionStatus`、`CuaPermissionObservationAttachment`）。这两个方法
**不能由 Rust Host 自足实现**：Helper launcher 必须是签名的 Electron 主进程（macOS 签名/ABI 门），
Windows 还需要按 Electron 版本锁定的 addon，`restartHelper` 经闭源 producer 的 resolver。

本 spec 的落点是：

- **Main 是 Helper 生命周期的唯一所有者**，并直接调用现有 CUA 运行时；
- **Rust Host 只做无状态转发**：`cua-permission` channel → `MainBroker.mainRequest` → Main；
- Host 不创建、不探测、不重启 Helper，也不保存任何 CUA 状态。

不在本 spec 范围：

- Agent 侧 CUA MCP/env 注入（`BROKER_SOCKET_ENV`、plugin authority）仍属 runtime 装配，另行立项；
- `cua-pip-session`（无 Renderer 调用，见边界 spec §2.2）；
- 远程 workspace 的 CUA（保持现有 `shouldCreateDefaultCuaProductHelper` 的 remote-identity 规则）。

## 2. 接口

Renderer 经 `@escode/rpc` 按位置参数调用：

```text
getStatus(workspacePath: string, workspaceIdentity?: string, options?: object)
restartHelper(workspacePath?: string, workspaceIdentity?: string, options?: object)
```

Host 转发给 Main 的 `mainRequest.params`：

```json
{
  "workspacePath": "/abs/workspace",
  "workspaceIdentity": "optional",
  "options": { "includeFunctionalProbes": true }
}
```

缺省字段必须**整个省略**（沿用 `forward.rs::runtime_target` 的严格省略规则，不写 `null`）。
`options` 原样透传：UI 实际发送 `includeFunctionalProbes`，不得归一化或删键。

Main 返回值：

- `getStatus`：`CuaPermissionStatusResult`（`available:false` 的 idle/未启用/平台不支持也用结果对象表达）；
- `restartHelper`：`CuaPermissionRestartResult`（`{ok:boolean, reason?:string}`）。

错误：Main 未声明 `mainServices` 时 Host 回 `NotImplemented`；校验失败回 `InvalidRequest`；
Main 抛错由控制通道转成 `{ok:false}`，Host 映射为 `Fatal`。两个方法本身不抛领域错误。

## 3. 所有者与事件顺序

```text
Renderer
  │  @escode/rpc  cua-permission.getStatus / restartHelper
  ▼
host-rpc (domains_cua_permission.rs)      无状态：只解析位置参数
  │  MainBroker.request("cua-permission.<method>", params)
  ▼
host-control MainRequest  ──stdio──►  Electron Main
                                        │  唯一 Helper 生命周期 owner
                                        │  createDefaultCuaProductHelper / resolver.restart()
  ◄──────────── MainResponse ───────────┘
  ▼
Renderer
```

规则：

1. Host 不缓存结果；每次调用都走 Main，`getStatus` 的探针语义由 Main 决定。
2. Main 的 CUA service 是**进程级单例**，与 Node Host 中的实现共用同一套底层 factory；
   同一 App 不出现两个 Helper 生命周期 owner。
3. `mainRequest` 无超时（`main_broker.rs`）：`getStatus` 可能冷启动 Helper（最长约 5 s），
   Renderer 侧沿用现有重试/保留上次状态的策略；Main handler 必须最终 resolve 或 reject。
4. Host 关闭控制通道时 pending 请求以 `Control channel closed` 收口，不静默挂起。

## 4. Main 侧实现

- 新增 `packages/services/src/cua-permission-broker/cuaPermissionServiceRuntime.ts`：
  - `createCuaPermissionService(deps)` 承载 `getStatus` / `restartHelper` 的全部编排；
  - `createStandaloneCuaPermissionService(options)` 供 Main 组装进程级单例；
  - Node Host（`node.ts`）与 Main 共用同一个工厂，避免两条实现路径。
- Main 新增 `packages/desktop/src/main/desktopCuaPermissionService.ts`：
  - 懒创建单例；`platform`/`env` 可注入；
  - 启动前确保 `ESCODE_CUA_LAUNCHER_PID = String(process.pid)` 对 Helper 可见；
  - `escode-host` 退出/App quit 时 dispose。
- `handleRustHostMainRequest` 增加两个 case，参数校验后调用单例。

### 4.1 与 Node Host 的已知差异

1. **活跃 turn 查询**：Node Host 把 `escodeAgentService.hasActiveCuaOperationTurn()` 前向引用给
   resolver；Main 看不到 Rust Host 内正在跑的 CUA turn。standalone 组装传入 `hasActiveTurn: () => false`。
   可见条件：用户在 CUA turn 进行中主动点“重启 Helper”；现有 UI 只在授权流程/设置页触发。
   后续由 Host 上报活跃 turn 时再消除。
2. **远程 workspace identity**：Main 组装按 `workspaceIdentity` 非空判定“非本地”，与
   `shouldCreateDefaultCuaProductHelper` 的现有规则一致；不为远程 workspace 创建本地 Helper。
3. **单例归属**：Node Host 与 Main 不会同时服务同一个窗口；切换期由
   `serviceAuthorityMode`/Host 开关决定。M5 删除 Node Host 后 Main 是唯一 owner。

## 5. 验收

- 契约：`cua-permission` 两个成员进入 `IMPLEMENTED`；`contract.rs` 双向对账通过。
- Rust 单测：
  - 无 `mainServices` → `NotImplemented`，且不发出 `MainRequest`；
  - `getStatus` 缺 `workspacePath` → `InvalidRequest`，不发出请求；
  - 位置参数转 `mainRequest.params` 时缺省字段省略；
  - Main 回 `ok:false` → `Fatal` + 原文案。
- Main 单测：参数校验、委托参数、缺省字段省略。
- 真实链路：`startRustHost({ onMainRequest })` + 真实 `@escode/client`，断言
  `getStatus`/`restartHelper` 的 method/params/返回值；无 `onMainRequest` 时拒绝。
