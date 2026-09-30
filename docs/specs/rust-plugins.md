# Rust 插件面（plugins/*）

2026-09-30。方法级 diff（见 rust-parity-remaining.md）发现：App 的插件管理 UI 直接调用
`plugins/list`、`plugins/setEnabled`、`plugins/overview`、`plugins/referenceCatalog(WithCategory)`，
而 Rust engine **一个方法字符串都没有**（`rg --fixed-strings` 全仓为 0）。

| 方法 | App 调用点 | Rust |
| --- | --- | --- |
| `plugins/list` | `packages/ui/src/store/pluginManagementStoreLoading.ts`、`RemotePluginSyncDialog.tsx`、`App.tsx`（pluginService 装配） | 无 |
| `plugins/setEnabled` | 插件页开关 | 无 |
| `plugins/overview` / `plugins/referenceCatalog*` | 插件页概览/引用目录 | 无 |

App 对 `plugins/list` 的错误只在**超时**时重试，method-not-found 直接上抛 → 用 Rust runtime 时插件页与
远端插件同步会失败。这不是「可选能力」，是 App 生命周期矩阵里的缺口。

## TS 基准与规模

- 协议面：`bootstrap/src/zcode-protocol/plugins.ts`（`listPlugins` / `setPluginEnabled` /
  `getPluginsOverview` / 引用目录 + `toPluginInfo` / `createMissingConfiguredPluginInfos` /
  `createPluginConfigView`）。
- 解析面：`bootstrap/src/plugins.ts::resolveZCodePlugins`（`discoverNodePluginsSync`）。
- 插件域实现：`packages/adapters/src/plugins/*`，约 **5.9k 行**（`marketplace.ts` 单文件 101 KB），
  含市场索引、zip/GitHub 源、原子目录安装、组件枚举、hooks、MCP、版本比较。

结论：这是一整块领域，不是「补一个方法」。本文件把它拆成分期，逐期对齐 + 差分，避免半成品列表冒充完成。

## 结果契约（`zcodePluginInfoSchema`）

- 必填：`id`、`name`、`enabled`、`source`、`marketplace`、`skillRootCount`、`commandRootCount`、
  `mcpServerNames`、`rootPath`。
- 可选：`description`、`version`、`author`、`authorUrl`、`homepage`、`skillCount`、`components`、
  `declaredMcpServerNames`、`hostMcpServerNames`、`hookDetails`、`userConfig`、`configuredOptions`、
  `packageStatus`、`rootSource`、`enabledSource`、`optionSources`。

`components` 是详情页的权威来源（skills/commands/agents/hooks/mcp 的名称 + 可选描述，与启用态无关）；
`userConfig`/`configuredOptions`/`optionSources` 驱动选项表单与来源标注。

## Rust 现状与差距

`crates/tools/src/extension_plugins.rs::enabled(cwd, config, cancel)` 已经做了一部分发现：
inline `plugins.dirs`、官方插件 seed（`official_plugins.rs`）、bundled-marketplace 缓存、
`installed_plugins.json`、`:market` 后缀 id、`plugins.suppressedBuiltins`、
`plugins.enabledPlugins[id] ?? default || plugin_defaults`。返回 `Plugin { id, root, name, manifest, official }`。

差距：

1. 只返回**已启用**插件，`plugins/list` 需要「全部已发现插件 + enabled 标志」。
2. 没有组件枚举（skills/commands/agents/hooks/mcp 的名称与描述）、没有 `hookDetails`。
3. 没有配置视图（`plugins.enabledPlugins` / `plugins.options` 及其 per-scope 来源
   `sources.plugins.enabled|options`），因此 `enabledSource`/`optionSources`/`configuredOptions`
   与 `createMissingConfiguredPluginInfos`（已声明但未物化的配置行）都缺。
4. 没有 `plugin.json` 的选项 schema（`userConfig`）与 `packageStatus`。
5. 没有市场元数据（`marketplace` 目前只能从 id 的 `@` 后缀推，TS 还会读市场索引里的 listing）。

已核对（实现前不必重查）：

- marketplace 常量两边一致：inline 目录 = `inline`（`ZCODE_INLINE_PLUGIN_MARKETPLACE`），
  官方 = `zcode-plugins-official`（`ZCODE_OFFICIAL_PLUGIN_MARKETPLACE`），Rust 现有 id 形如
  `name@<marketplace>` 与 TS 相同，所以 `enabledPlugins` / `suppressedBuiltins` 的键能对上。
- defaultEnabled：inline 目录 = true，官方 root/cache = false，市场安装（`installed_plugins.json`）= false；
  实际值 = `plugins.enabledPlugins[id] ?? defaultEnabled || plugin_defaults 含该 id`（Rust 已按此实现）。
- `source` 取值：inline 目录 `"inline"`、官方 `"official"`、市场缓存 `"cache"`，另有 `"missing"`
  （仅用于 `createMissingConfiguredPluginInfos` 生成的占位行）。
- `skillRootCount` / `commandRootCount` = 组件根数量（默认目录 + manifest 声明的根），
  `skillCount` = 这些根下 SKILL.md 的数量（不是声明数）。

## 分期

1. **发现层 + `plugins/list`（读）**：拆出 `all()`（带 `enabled`/来源），输出全部必填字段 +
   `description/version/author/authorUrl/homepage`（manifest）+ `skillCount/skillRootCount/commandRootCount`
   （复用现有 skill/command 根枚举）+ `components`（复用现有 frontmatter 解析器）+ `declaredMcpServerNames`
   / `mcpServerNames`（复用 mcp 配置发现）。不输出选项面与 hooks 明细（可选字段，缺失好过伪造）。
2. **`plugins/setEnabled`（写）**：`enabledPlugins[id]` 写 user/workspace 配置 + 回写 `enabledSource`，
   并与现有配置合并/原子写一致（不能只改内存）。
3. **`plugins/overview` + `plugins/referenceCatalog(WithCategory)`**：市场/引用目录读面，依赖市场索引。
4. **选项面**：`userConfig`（plugin.json 选项 schema）、`configuredOptions`、`optionSources`、
   `hookDetails`、`packageStatus`（保留已声明但目标 Host 未物化的配置行）。

市场安装/更新/同步（`marketplace.ts` 的写面）不在本分期；Rust 先只做「读 + 开关」。

## 验收

- 每期都要有 App 差分：同一 fixture 下写入 `plugins.dirs` + `enabledPlugins` 配置，
  比对 Node 与 Rust 的 `plugins/list`（字段逐个，含顺序与可选字段的缺席语义）。
- 覆盖：enabled / 被 `enabledPlugins:false` 关闭 / `suppressedBuiltins` / 缺失配置行
  （`packageStatus: "missing"` + `source: "missing"`）/ inline 目录的 `rootSource`。
- `plugins/setEnabled` 差分要断言落盘的配置文件内容与随后 `plugins/list` 的可观察结果一致。
- 日志、诊断（`diagnostics`）文案与严重级别与 Node 一致；不写凭据或真实用户路径。

## 实现与验证（第 1 期，2026-09-30）

已落地 `plugins/list` 读面：

- `extension_plugins` 拆成 `all()`（全部已发现插件 + `enabled` 标志）与 `enabled()`（过滤，行为不变）；
  `Plugin` 增加 `marketplace` / `source` / `enabled`。
- `crates/tools/src/plugin_list.rs`：按 TS `toPluginInfo` + `createMissingConfiguredPluginInfos` 输出
  `id/name/enabled/source/marketplace/rootPath/skillCount/skillRootCount/commandRootCount/`
  `declaredMcpServerNames/mcpServerNames` + manifest 的 `description/version/author/authorUrl/homepage`。
  计数口径与 TS 相同：`resolveComponentRoots`（默认目录 + manifest 路径，去重）+ `skillCount` 为根下
  SKILL.md 数（两级扫描、不跟随符号链接）+ manifest `commands` 为对象时额外 +1 个生成根；
  停用插件走「计数 0 + MCP 名空」，与 TS `emptyComponents` 一致；配置里声明但未发现的行补
  `source: "missing"` + `packageStatus: "missing"`。
- `configScope: "user"` 只读用户层配置（`extension_config::load_user`）。
- MCP 名复用运行时同一套解析：`mcp_config::plugin_definitions`（`.mcp.json` + manifest）与
  `plugin_servers`（命名空间 `plugin:<name>:<key>`、鉴权/失效判定）。

仍属后续期（schema 可选项，缺失不伪造）：`components` 分组、`userConfig`、`configuredOptions`、
`optionSources`、`enabledSource`、`rootSource`、`hookDetails`（第 4 期）；`plugins/setEnabled`
（第 2 期）；`plugins/overview` / `referenceCatalog`（第 3 期）。

验收：`packages/services/tests/zcode-cli-rust-plugins-list.test.ts`——同一 fixture 下
（inline 两个插件根 + `enabledPlugins` 关闭其中一个 + 一条只声明未安装的 id），Node 与 Rust 在
上述字段上逐值一致，且三行都带 schema 必填字段。

差分中观察到、需单独确认的一点：该 fixture 里 Node 的列表还包含
`node-repl-host@zcode-plugins-official` 与 `browser-use@zcode-plugins-official`（来自随包官方插件根），
Rust 这次 seed 后没有产出这两行。官方插件的发现来源（随包根 vs storage 缓存 vs App 安装）需要单独
对齐后再纳入断言，本次比对按 id 前缀过滤，未把该差异写成期望。
