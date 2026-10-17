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
3. ~~超时重试~~ 已补（§6）：仅 `listPlugins` 超时后回收管理面实例并重试一次。
4. **进度事件源**：来自管理面实例的 `plugins/operationProgress` 通知；payload 原样转发（TS 经 zod 校验丢弃不合法者）。

## 5. 验收

- `plugin_params` 单测：整形表逐行断言（条件展开的边界值：`dryRun:false`、`operationId:""`、`clearOptionKeys:[]`、`null`）。
- 集成（真实内嵌 CLI runtime）：`listPlugins` / `getPluginsOverview` / `describePlugin`（不存在的插件）经 RPC 得到与直接调用 runtime 相同的结果；
  `cancelPluginOperation` 未知操作回 runtime 的应答；`getPluginReferenceCatalog` 走 workspace 实例；缺 dataBaseDir 回显式缺口。
- 进度扇出：`PluginHub` 单测（按 `operationId` 分发、非进度通知与坏帧忽略、断开的监听者被清理）。端到端进度只有 `plugins/resolveSuggestedReference`
  未命中本地目录时才会发（要联网刷新官方目录），不进离线测试；订阅先于请求的顺序由 `attach` 在 `management()` 里的位置保证。
- 去掉 `attach` 前置 / 改坏整形表任一行，对应测试失败。

## 6. stale client 恢复（`listPlugins`，2026-10-16）

TS `listPlugins` 用默认协议超时（30 s，而非 5 分钟）发请求；超时后 process manager 回收无响应的 client，**重试一次**，让设置页能从重新拉起的 app-server 自动恢复。

```
listPlugins ──▶ 管理面实例 ──(30 s 无应答: RequestTimeout)──▶ 回收实例（Instances.close：收口当前世代）
                                                                  │
                                      management() 懒启动新一代 + Hub 重新 attach ──▶ 重试一次 ──▶ 结果/原样报错
```

- **失败分类**：`host-runtime` 的 `RequestTimeout`（类型化错误，消息不变）与“实例已关闭 / runtime 错误应答”区分；`forward::request_classified` 返回 `CallError::{Timeout, Failed}`，对外仍是同一个 `RpcError`。
- **只对幂等只读请求开启**：`Spec::retry_stale` 仅 `listPlugins` 为真（单测断言集合恰为 `["listPlugins"]`，安装/卸载/配置等写操作重试会重复副作用）。
- **次数**：回收一次、重试一次；第二次仍超时原样报错，不再重试；非超时失败（含 runtime 错误应答、实例已关闭）既不回收也不重试。
- **所有者**：回收由 `plugin-management` 的 `Context` 做（`Instances` 是实例表唯一所有者）；进度 `Hub` 在新一代实例上由 `management()` 里的 `attach` 重新订阅，订阅先于请求的顺序不变。

**验收**：`domains_plugin_tests`（先超时后成功：回收 1 次且先于重试；连续超时：恰 2 次尝试；非超时失败：不重试不回收；`retry_stale` 集合）；
`host-runtime/tests/instances.rs`（真实嵌入 runtime：零期限必然超时且可 downcast 为 `RequestTimeout`，实例收口后的失败不是超时）。
负向验证：去掉回收调用 → “先超时后成功”用例失败。

**仍登记的差异**：其余结果的 schema 校验与进度 payload 校验未做（见 §7）。

## 7. 结果 schema 校验（`plugins/list`、`plugins/overview`，2026-10-16）

**更正（§6 旧说法）**：这两个结果的 zod schema 全部是 `.strict()`——未知键**直接拒绝**，不是剥掉；也没有 `default/transform`。
所以等价移植只是一个**纯接受/拒绝校验器**，成功时应答原样透传（与 TS 返回的 `parse` 结果逐值相同）。

**范围（2026-10-16 已扩到 `SPECS` 全部 16 个管理面方法）**：`escodePlugins{List,Overview,MarketplaceMutation,Install,CancelOperation,Uninstall,RestoreBuiltin,Configure,Validate,Describe,SetEnabled,ResolveSuggestedReference}ResultSchema`（install/update、configure/resetConfig、marketplace add/remove/update 各共用一张表），含它们依赖的子 schema
（plugin info / component group / hook detail / user config option / diagnostic / marketplace summary / available & installed summary / store listing / scope / option value）。
`getPluginReferenceCatalog`（新旧两个方法名共用 `escodePluginsReferenceCatalogResultSchema`）也已覆盖，但它是唯一**带 `.default(...)`** 的 schema：条目缺 `subagentNames` 时 TS 的 `parse` 会补 `[]`，所以校验通过后再跑 `normalize` 补默认值（`null` 不是缺省，照样拒绝）。
新方法名的校验失败是协议漂移而非“方法不存在”：**不回退旧名**（只有 `-32601` 才回退，TS 的 `ZodError` 没有 `code`）。
其余 schema 的 `parse` 输出等于输入——生成器对每条被接受的语料逐条断言这一点，论断因此是被检验的事实而非假设。
建议引用结果的 `.superRefine`（`ready/disabled/missing` 必须带 `marketplace`、`pluginName` 与 `sourceTrust:"official"`）用 `Ty::Check` 表达。

**行为**：runtime 应答 `result` 先过校验，失败 → `RpcError("Error", "ESCode Protocol response parse failed: <method>: <path>: <reason>")`（TS 抛 `ZodError`，其 message 是 issue 列表 JSON）。
校验在 stale 重试**之后**、结果返回之前；校验失败不是超时，不触发回收/重试。

**规则对照（zod 4.6.5）**：`nonEmptyString` = `trim().min(1)`，`trim` 用 **JS 的空白集**（含 U+FEFF、不含 U+0085），不用 Rust 的 `str::trim`；`.int()` = 安全整数（`|n| ≤ 2^53-1` 且无小数部分，JSON 里的 `1.0` 算整数）；
`.nonnegative()` ≥ 0，`.positive()` > 0；`z.record(z.string(), T)` = 对象且每个值满足 `T`；`z.unknown()` 的 `record` = 任意对象（数组/`null` 都不是）；
`.strict()` = 出现未声明键即失败；`.optional()` = 键缺省可，**`null` 不可**；`z.literal`/`z.enum`/`z.union` 逐值判定。

**所有者**：`plugin_result_schema.rs`（声明式小 DSL，一处定义）；`Spec::validated(schema)` 把方法挂到 schema 上，`domains_plugin` 是唯一调用点。

**验收（TS 为 oracle）**：`scripts/generate-escode-host-plugin-schema-fixtures.mjs` 用**真实 zod schema**对一组有效样本及其系统化变异（逐路径：删键、换类型、加未知键、空串/纯空白/JS 特有空白、负数/小数/0/超安全整数、`null`）
求 `safeParse().success`，写入 `plugin_schema_golden.json`（`--check` 并入 `pnpm check:escode-host`，防漂移）；Rust 单测逐条断言 `validate` 的接受/拒绝与 TS 一致。
负向验证：放宽 `.strict()`、把 JS 空白集换成 `str::trim`、`optional` 接受 `null`，对应用例失败。

**登记差异（不修）**：错误文案（Rust 一句话含路径；TS 为 `ZodError` issue JSON）；差分只比较接受/拒绝，不比较 message。

**§7 交付记录（2026-10-16）**：`plugin_result_schema.rs`（DSL + 校验器）、`plugin_result_tables.rs`（`plugins/list`、`plugins/overview` 的 schema 声明表）、`domains_plugin::check_result`（唯一调用点）；
`scripts/generate-escode-host-plugin-schema-fixtures.mjs` 导出 5608 条判定（1426 条接受）为“配方”而非完整值（资产 290 KB，Rust 侧重放同一套变异），`--check` 已并入 `pnpm check:escode-host`。
验收：全部 5608 条判定与真实 zod 一致；真实嵌入 runtime 的 `listPlugins` / `getPluginsOverview` 应答通过校验（`rpc_plugin_management` 集成测试，说明现状没有契约漂移）；
JSON 里 `3.0` 算整数、`3.5` 不算（语料里 `JSON.stringify` 表达不了 `3.0`，由单测单独守住）。
负向验证（三项都被语料抓到）：放宽 `.strict()`、用 `str::trim` 代替 JS 空白集（两个方向都出现差异：JS 的 U+FEFF/Rust 的 U+0085）、让 `optional` 接受 `null`。
**覆盖守卫**：单测 `every_management_method_has_a_result_schema` 断言 `SPECS` 里每个方法都登记了 schema，新增方法忘了登记会直接失败；语料现为 13870 条判定（3493 条接受）加 3 条归一化样本。
**未覆盖**：进度 payload 仍原样透传。

## 8. 旧 `plugins` 通道（7 个成员）：整套退役（2026-10-17，已交付）

`plugins` 是 `plugin-management` 的**前身**，TS 侧已退役：`packages/services/src/plugins/pluginsService.ts`
（86 行）无条件返回固定应答或抛错，**不读参数、不接运行时、不触文件**（形参前缀 `_params`，连校验都没有）。
装配点 `packages/services/src/node.ts:2370` 的 `createPluginsService({ isDesktopRuntime: true })` 是无条件的。

| 成员                                                                                                                    | 行为                                                                                                                                  |
| ----------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| `getOverview`                                                                                                           | 固定回 `{marketplaces: [], availablePlugins: [], installedPlugins: [], capability: {supported: false}}`（TS `createRetiredOverview`） |
| `addMarketplace` / `removeMarketplace` / `updateMarketplace` / `installPlugin` / `uninstallPlugin` / `setPluginEnabled` | 抛 `Error("Legacy plugin management has been retired in ESCode Agent mode")`（TS `throwRetiredPluginManagement`）                     |

**实现**：`host-rpc/src/domains_plugins_legacy.rs`（纯常量表，无状态、无依赖、不解析参数）。
参数一律忽略——TS 忽略参数，故「缺 `workspacePath`」也必须照常回常量 / 抛退役错，**不能**回 `InvalidRequest`；
这与「新通道缺参回 InvalidRequest」的惯例相反，是本通道的登记语义。

**登记差异**：无。错误名 `Error`、文案与 `createRetiredOverview` 的四个字段逐字对齐；
TS 的 `isDesktopRuntime` 选项对本通道行为无影响（`_options` 未使用），Rust 侧不引入对应开关。

**验收**：单测逐条断言 7 个成员的应答 / 错误文案与常量字段；契约清单登记该 channel 的 7 个成员。
负向验证：改退役错文案一个字、把 `capability.supported` 改成 `true`，对应用例失败。
