# Rust 插件面（plugins/*）

2026-09-30。方法级 diff（见 rust-parity-remaining.md）发现：App 的插件管理 UI 直接调用
`plugins/list`、`plugins/setEnabled`、`plugins/overview`、`plugins/referenceCatalog(WithCategory)`，
而 Rust engine **一个方法字符串都没有**（`rg --fixed-strings` 全仓为 0）。

| 方法                                             | App 调用点                                                                                                             | Rust          |
| ------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------- | ------------- |
| `plugins/list`                                   | `packages/ui/src/store/pluginManagementStoreLoading.ts`、`RemotePluginSyncDialog.tsx`、`App.tsx`（pluginService 装配） | 第 1 期已实现 |
| `plugins/setEnabled`                             | 插件页开关                                                                                                             | 第 2 期已实现 |
| `plugins/overview` / `plugins/referenceCatalog*` | 插件页概览/引用目录                                                                                                    | 第 3 期已实现 |

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
  `declaredMcpServerNames/mcpServerNames/components` + manifest 的
  `description/version/author/authorUrl/homepage`。
  计数口径与 TS 相同：`resolveComponentRoots`（默认目录 + manifest 路径，去重）+ `skillCount` 为根下
  SKILL.md 数（两级扫描、不跟随符号链接）+ manifest `commands` 为对象时额外 +1 个生成根；
  停用插件走「计数 0 + MCP 名空」，与 TS `emptyComponents` 一致；配置里声明但未发现的行补
  `source: "missing"` + `packageStatus: "missing"`。
- `components` 与启用态无关（TS `createPluginMetadata` 始终对插件根做权威枚举）：分组顺序
  agent → command → skill → mcp，名字/描述取 manifest 声明或 frontmatter（`name`/`description`，
  按 TS `parseScalar` 去引号），命令/技能分别按「名字」与「文件路径 + 名字」去重。
- `configScope: "user"` 只读用户层配置（`extension_config::load_user`）。
- MCP 名复用运行时同一套解析：`mcp_config::plugin_definitions`（`.mcp.json` + manifest）与
  `plugin_servers`（命名空间 `plugin:<name>:<key>`、鉴权/失效判定）。

仍属后续期（schema 可选项，缺失不伪造）：`components` 的 **hook 分组**（TS 走 loader 的 hook 源发现
——manifest hooks + hook 文件且要过 `canRunPluginHooks`，直接读 manifest 键会输出 Node 不会显示的名字，
故留到第 4 期与 `hookDetails` 一起做）、`userConfig`、`configuredOptions`、`optionSources`、
`enabledSource`、`rootSource`；`plugins/setEnabled`（第 2 期）；`plugins/overview` /
`referenceCatalog`（第 3 期）。

验收：`packages/services/tests/zcode-cli-rust-plugins-list.test.ts`——同一 fixture 下
（inline 两个插件根 + `enabledPlugins` 关闭其中一个 + 一条只声明未安装的 id，含 agents/commands/
skills/`.mcp.json` 组件），Node 与 Rust 在整份列表的上述字段（含 `components`）上逐值一致，
且所有行都带 schema 必填字段；停用插件的 `components` 仍然完整而计数为 0。

官方插件行的说明（已查清，不是缺口）：Node 从随包官方插件根直接发现，Rust 依赖
`official_plugins::seed_once` 把同一份随包内容写进 `<storage>/cache/zcode-plugins-official`。用例里
必须像 App 的 `zcodeAgentProcessManager` 那样注入 `ZCODE_OFFICIAL_PLUGINS_BASE_DIR`（随包目录）、
`ZCODE_PLUGIN_HOST_EXEC_PATH` 与 `ZCODE_PLUGIN_HOST_ENTRYPOINT`，否则 Rust 无源可 seed、会少
`node-repl-host@zcode-plugins-official` 与 `browser-use@zcode-plugins-official` 两行。注入后两侧
**整份列表**（含官方插件、含顺序）在上述字段上一致。

## 实现与验证（第 2 期，2026-10-02）

`plugins/setEnabled` 写面已落地（`crates/tools/src/plugin_list.rs::set_enabled` + `config_file.rs`），
对齐 TS `setPluginEnabled` → `setZCodePluginEnabled` → `updatePluginEnabledInFileConfig`：

- 选择器（TS `resolvePluginSelector`）：`trim` 后先按完整 id，再按唯一 `name`；重名报
  `Plugin name is ambiguous, use full plugin id: …`，否则 `Plugin not found: …`。范围是**已发现**插件
  （配置视图含项目层），所以只在配置里声明、未安装的 missing 行同样报未找到。
- 写入目标（TS `resolvePluginConfigPath`）：`scope: "workspace"` 固定 `<workspacePath>/.zcode/config.json`
  （不走 project discovery 的最外层文件），否则用户层 `~/.zcode/cli/config.json`。
- 补丁（TS `patchPluginEnabled`）：只动 `plugins.enabledPlugins`，先删掉 id 的别名（CUA 旧 id
  `zcode-cua@zcode-plugins-official` → `computer-use@zcode-plugins-official`），再把规范 id 追加到末尾；
  `plugins` / `enabledPlugins` 不是对象时原位替换为对象。
- 落盘（TS `atomicWriteJson`）：同目录临时文件（unix 0600）+ rename，失败清理临时文件；内容为
  `JSON.stringify(value, null, 2) + "\n"`。Rust 的 serde_json 没开 `preserve_order`（全局打开会改变
  其它输出的 key 顺序），因此复用 `domain::json_order::Json`（保序；重复 key 后者覆盖但保持首次位置）。
  顺带把它的数字输出补齐为 JS `Number#toString`（先按 f64 取值：`1.0` → `1`、超出 2^53 的整数按 f64
  舍入；`1e21` → `1e+21`），工具 input 转写与官方插件缓存这两个既有调用方同样受益。
  读失败 / 非 JSON / 非对象的错误文案与 TS 相同；文件不存在视为 `{}`。
- 返回值：与 TS 一样基于**写入前**解析的插件元数据（停用 → 启用时计数仍为 0，等下一次 `plugins/list`
  刷新），覆盖 `enabled`，并带 `enabledSource = scope ?? "user"`。
- 生效时机：只改配置文件；运行中会话的插件集合不变，新会话按新配置装载（与 TS 冻结的 session catalog 一致）。

未对齐（记录在案）：TS `createConfig` 装载时会把旧 CUA key 迁移并回写磁盘（`persistPluginConfigMigration`），
Rust 读路径不做这一步；只影响仍保存旧 id 的历史配置，写入时的别名清理已对齐。

验收：`packages/services/tests/zcode-cli-rust-plugins-set-enabled.test.ts`——同一 fixture 下比对 Node 与
Rust 的：按 id 启用（user 层）、按带空格的 name 停用（workspace 层，文件原本不存在）、未知 id、missing 行
四次调用的返回/错误文案，两份配置文件写后的**完整字节**（含补丁外 key 的顺序、`1.0` 的排版），以及随后
`plugins/list` 的启用态。

## 实现与验证（第 3 期 · overview，2026-10-02）

`plugins/overview` 已落地（`crates/tools/src/plugin_overview.rs` 组装 + `plugin_marketplace.rs` 存储解析），
对齐 TS `getPluginsOverview` → `getZCodePluginsOverview`：

- 每次调用先做 TS `ensureDefaultPluginMarketplaces`：`known_marketplaces.json` 缺官方市场记录时补一条
  （`source: {source:"url", url:<CDN>}`、`addedAt` 为当前 ISO 时间、`pluginCount: 0`）并整份重写
  `{version:1, marketplaces}`——已有记录保序原样、未过 `isKnownMarketplaceRecord` 的记录被丢弃（与 TS 相同）。
- 市场：`known` + 用户层 `plugins.extraKnownMarketplaces` 声明（项目层声明被 TS config-merger 丢弃；
  file/directory 相对路径按 `~/.zcode/cli` 解析）。同 id 同 source 读缓存 manifest；异 source 的非官方声明替换成
  不读缓存的占位记录（`pluginCount: 0`）；官方 id 是保留身份，声明只产生
  `plugin_marketplace_declaration_reserved` 诊断。摘要的 `pluginCount` 取 manifest 可见条目数（官方市场排除
  `node-repl-host`），无 manifest 时取记录值；`featured` 只收非空字符串。
- `availablePlugins`：manifest 条目（数组或 name → entry 对象两种写法，名字 trim 后非空），`componentTypes`
  按条目原样 key 推断，`listing` 按 TS `parseEntryStoreListing`。
- `installedPlugins`：`installed_plugins.json`（数组逐条校验；Claude 风格对象写法归一化）；`enabled` 取配置
  `enabledPlugins[id] ?? false`；已被发现层加载时取 manifest 的 description/version 与
  `inferComponentTypesFromMetadata`；更新判定按 TS `comparePluginUpdate`（目录条目有 version 用 semver
  `coerce` 比较，否则比 source pin：zip sha256 > sha > commit），`latestVersion` 为 version 或 sha 前 7 位；
  按 id join 目录条目的 listing。
- `restorableBuiltins`：被 `suppressedBuiltins` 抑制的官方定义（computer-use 另需 CUA 特性，同 TS
  `isZCodeCuaInternalFeatureEnabled`），listing 取定义 seed。
- 诊断：声明保留 id + 市场 `lastRefreshFailure`（severity error）。

未对齐（记录在案，均为 schema 可选或独立缺口）：

- **发现层诊断**（`plugin_root_not_found` 等，来自 TS `discoverNodePluginsSync`）Rust 不产出——`plugins/list` 的
  `diagnostics` 同样缺。这是发现层的独立缺口，下一步单独补，届时 list/overview 一并对齐。
- `installedPlugins[].hookDetails` 与 componentTypes 里的 `hook` 依赖 hook 源发现（第 4 期）。
- 读路径不做 TS `recoverAtomicTargetSync` 的崩溃恢复：Rust 不写 marketplaces/cache 目录，Node writer 崩溃留下的
  事务残留由 Node 下次操作恢复；在此之前 Rust 可能读到旧一代或缺失的 manifest。

验收：`packages/services/tests/zcode-cli-rust-plugins-overview.test.ts`——同一 fixture（第三方市场含 listing /
featured / 版本更新 / sha 更新 / zip 源、刷新失败市场、非法记录、用户层声明含保留官方 id 与相对目录、已安装
插件含真实根与缺失根、被抑制的官方插件）下，Node 与 Rust 的整份结果逐值一致（剔除 hookDetails 与发现层诊断），
且 overview 补写后的 `known_marketplaces.json` 一致（剔除 `addedAt`）。

## 实现与验证（第 3 期 · referenceCatalog，2026-10-02）

`plugins/referenceCatalog` / `plugins/referenceCatalogWithCategory` 已落地（`crates/tools/src/plugin_reference.rs`

- `crates/core/src/app/plugin_catalog.rs`），对齐 TS `getPluginReferenceCatalog` + core `buildPluginReferenceCatalog`：

* 身份条目：全部已发现插件（含停用）；`conflictingPluginIds` = 同 manifest name 的其它**启用**插件（停用条目
  不参与、为空）；`skillQualifiedNames` / `subagentNames` = `<plugin name>:<组件名>`（来自与启用态无关的组件
  枚举，去重排序）；`mcpServerNames` 排序（停用插件为空）。`rootPath` 不出协议。
* 展示条目：按 overview 的 available → installed → restorable 依次合并 listing（`category` / `icon` /
  `displayName` / `displayNameI18n` / `descriptionI18n`）与 `description`（trim 后非空才覆盖）；
  `WithCategory` 额外带 `category`，缺省 `"other"`。展示部分每次现取，不随会话冻结。
* 权威：不带 `sessionId` → `authority: "workspace"` 现算；带 `sessionId` → `authority: "session"`，会话不存在
  （或已关闭）fail closed，不回退 workspace。

与 TS 的差异（记录在案）：TS 在 App（会话运行时）创建时冻结身份目录、冷恢复重建；Rust 在该会话**首次**查询
引用目录时冻结（engine 内存，会话关闭即丢弃，不落盘）。只有「会话创建后、首次打开 Picker 前」改了插件配置时
两者可观察不同。

验收：`packages/services/tests/zcode-cli-rust-plugins-reference-catalog.test.ts`——inline 插件含 skill/agent/MCP、
停用插件、inline 与市场安装的同名插件冲突、市场 listing 展示 join；比对 workspace / WithCategory / 会话首次 /
`setEnabled` 之后的会话（仍为冻结值）与 workspace（已更新）/ 未知会话报错，Node 与 Rust 逐值一致。
