# Rust Host `plugin-management` 域（U2）

> 状态：设计 + 交付 · 2026-10-15。TS 真相源：`packages/services/src/plugins/pluginManagementService.ts`（薄转发）与
> `packages/services/src/escode-agent/escodeAgentService.ts` 的 `listPlugins` … `setPluginEnabled`（4330–4400、4800–5000 行）。

## 1. 范围（18 个成员）

`plugin-management` channel：`listPlugins`、`getPluginsOverview`、`addPluginMarketplace`、`removePluginMarketplace`、`updatePluginMarketplace`、
`installPlugin`、`cancelPluginOperation`、`uninstallPlugin`、`updatePlugin`、`restoreBuiltinPlugin`、`configurePlugin`、`resetPluginConfig`、`validatePlugin`、
`describePlugin`、`setPluginEnabled`、`resolveSuggestedPluginReference`、`getPluginReferenceCatalog`，事件 `onDynamicPluginOperationProgress(operationId)`。

插件的事实源在 CLI runtime（读写 `~/.escode` 插件目录并热更新运行态），Host 无副本——Host 的职责只有**参数整形 + 路由 + 超时 + 进度事件扇出**。

## 2. 所有者与路由

```
Renderer ──RPC──▶ plugin-management.<method>(params)
                    │ 整形（逐条对照 TS：条件展开、undefined 省略、null 保留）
                    ▼
        ┌─ 管理面实例（15 个方法 + 进度事件）：workspace = <dataBaseDir>/.escode/plugin-workspace
        │      TS：专用 pluginProcessManager 的单个进程；Rust：Instances 里一个固定 identity 的实例
        └─ workspace 实例（getPluginReferenceCatalog）：params 的真实 workspace
                    ▼
              runtime 请求 plugins/*   ←── runtime 通知 plugins/operationProgress
                                              │
                          PluginHub（管理面实例的唯一帧订阅者，按 operationId 扇出）
```

- 请求体里的 `workspace` 引用是调用方的**真实** workspace（TS `buildWorkspaceRef(params)`），runtime 据此做 workspace-scope 判定；管理面实例只是承载进程。
- **进度竞态的所有权**：`PluginHub::listen(operationId)` 同步登记；管理面实例每次被取用都先 `attach`（订阅其帧流）再返回，所以任何请求发出前订阅已就位，
  `onDynamicPluginOperationProgress` 在请求之前登记就不会丢首帧。实例重启（新世代）后下一次取用重新 attach。
- `cancelPluginOperation` 只带 `{operationId}`，必须落在发起操作的同一进程——管理面单实例天然满足。

## 3. 整形规则（对照 TS）

| 方法                                        | runtime 方法                                      | 请求体                                                                              | 超时  |
| ------------------------------------------- | ------------------------------------------------- | ----------------------------------------------------------------------------------- | ----- |
| listPlugins / getPluginsOverview            | `plugins/list` / `plugins/overview`               | `workspace`, `configScope?`                                                         | 30 s  |
| addPluginMarketplace                        | `plugins/marketplace/add`                         | `workspace`, `source`, `dryRun?`(≠undefined), `operationId?`(真值)                  | 5 min |
| removePluginMarketplace                     | `plugins/marketplace/remove`                      | `workspace`, `marketplace`                                                          | 5 min |
| updatePluginMarketplace                     | `plugins/marketplace/update`                      | `workspace`, `marketplace?`, `operationId?`                                         | 5 min |
| installPlugin                               | `plugins/install`                                 | `workspace`, `pluginName`, `marketplace`, `scope?`, `dryRun?`, `operationId?`       | 5 min |
| cancelPluginOperation                       | `plugins/cancelOperation`                         | `{operationId}`（无 workspace）                                                     | 5 s   |
| uninstallPlugin                             | `plugins/uninstall`                               | `workspace`, `pluginId?`, `pluginName?`, `marketplace?`, `removeCache?`(≠undefined) | 5 min |
| updatePlugin                                | `plugins/update`                                  | `workspace`, `pluginId?`, `marketplace?`                                            | 5 min |
| restoreBuiltinPlugin                        | `plugins/restoreBuiltin`                          | `workspace`, `pluginId`                                                             | 5 min |
| configurePlugin                             | `plugins/configure`                               | `workspace`, `pluginId`, `options`, `clearOptionKeys?`(非空), `scope?`, `dryRun?`   | 5 min |
| resetPluginConfig                           | `plugins/resetConfig`                             | `workspace`, `pluginId`, `scope?`                                                   | 5 min |
| validatePlugin                              | `plugins/validate`                                | `workspace`, `pluginName?`, `marketplace?`, `source?`                               | 5 min |
| describePlugin                              | `plugins/describe`                                | `workspace`, `marketplace`, `pluginName`                                            | 5 min |
| setPluginEnabled                            | `plugins/setEnabled`                              | `workspace`, `pluginId`, `enabled`, `operationId?`, `scope?`                        | 5 min |
| resolveSuggestedPluginReference             | `plugins/resolveSuggestedReference`               | `workspace`, `stableId`, `operationId`, `clientMode`, `deliveryKind`（缺省的省略）  | 5 min |
| getPluginReferenceCatalog（workspace 实例） | `plugins/referenceCatalogWithCategory` → 回退旧名 | `workspace`, `sessionId?`；仅当错误码 `-32601` 才回退 `plugins/referenceCatalog`    | 30 s  |

“可选(真值)”= 值为 `undefined/null/""/false/0` 时省略（TS 的 `...(x ? {…} : {})`）；“(≠undefined)”= 只要键存在（含 `null`/`false`）就带上。

## 4. 登记差异（不修）

1. **不做结果 schema 校验**：TS 用 zod 校验 runtime 应答；Rust 原样透传 `result`（runtime 与 Renderer 都是同一仓库的 Rust/TS 合同）。
2. **承载进程**：TS 管理面是 `pluginProcessManager` 的专用进程；Rust 是 `Instances` 里 identity=`<dataBaseDir>/.escode/plugin-workspace` 的实例（同一 cwd，生命周期随 Host）。
3. **超时重试**：TS 对 `listPlugins` 超时会回收 stale client 并重试一次；Rust 实例没有该回收路径，超时直接报错（实例退出由 `Instances` 懒重启接管）。
4. **进度事件源**：来自管理面实例的 `plugins/operationProgress` 通知；payload 原样转发（TS 经 zod 校验丢弃不合法者）。

## 5. 验收

- `plugin_params` 单测：整形表逐行断言（条件展开的边界值：`dryRun:false`、`operationId:""`、`clearOptionKeys:[]`、`null`）。
- 集成（真实内嵌 CLI runtime）：`listPlugins` / `getPluginsOverview` / `describePlugin`（不存在的插件）经 RPC 得到与直接调用 runtime 相同的结果；
  `cancelPluginOperation` 未知操作回 runtime 的应答；`getPluginReferenceCatalog` 走 workspace 实例；缺 dataBaseDir 回显式缺口。
- 进度扇出：`PluginHub` 单测（按 `operationId` 分发、非进度通知与坏帧忽略、断开的监听者被清理）。端到端进度只有 `plugins/resolveSuggestedReference`
  未命中本地目录时才会发（要联网刷新官方目录），不进离线测试；订阅先于请求的顺序由 `attach` 在 `management()` 里的位置保证。
- 去掉 `attach` 前置 / 改坏整形表任一行，对应测试失败。
