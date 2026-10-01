# Rust 插件市场写面（install / uninstall / update / marketplace*）

2026-10-02。插件读面（list / setEnabled / overview / referenceCatalog / configure / resetConfig）已对齐
（[rust-plugins.md](rust-plugins.md)）。App 插件商店的**安装、卸载、更新、恢复内置、市场增删刷新**仍全部只在
Node 上可用：Rust 对这些方法回 method-not-found，用户在 Rust 会话里点「安装」会直接失败。本文件拆分这块写面。

## TS 基准

- 协议：`bootstrap/src/zcode-protocol/plugins.ts`（`installPlugin` / `uninstallPlugin` / `updatePlugin` /
  `restoreBuiltinPlugin` / `addPluginMarketplace` / `removePluginMarketplace` / `updatePluginMarketplace` /
  `validatePlugin` / `describePlugin`）、`plugin-reference-catalog.ts::resolveSuggestedPluginReference`。
- 核心：`bootstrap/src/plugins.ts::installZCodeMarketplacePlugin` / `uninstallZCodeMarketplacePlugin` /
  `restoreBuiltinPlugin`；存储：`adapters/src/plugins/marketplace.ts`（约 2.7k 行）、`atomic-directory.ts`、
  `zip-source.ts`、`github-archive-source.ts`。
- 并发：`withPluginStorageLock` 只是**进程内**按 storageRoot 串行（TS 明确不做跨进程锁）→ Rust 用同等的
  per-storage async 锁即可。

## 关键语义（已核对）

- 安装：`ensureDefaultPluginMarketplaces` → （声明市场按需物化）→ `installMarketplacePlugin`：解析依赖闭包
  （`allowCrossMarketplaceDependenciesOn`）→ 逐个 `cacheMarketplacePlugin`（解析源根 → 读真实版本
  `resolveInstalledPluginVersion` → 原子激活到 `cache/<market>/<name>/<version>`，内置 filesystem/sea 源的 cachePath
  即目标时不拷贝 → 补 manifest）→ 写 `installed_plugins.json`；任何一步失败按逆序回滚已激活目录。
- 安装后：官方市场清掉同 id 的 `suppressedBuiltins`；`enablePluginsByDefaultInFileConfig` 只给**用户配置里尚未
  显式声明**的 id 写 `true`（停用后重装不被覆盖）；scope 一律按 user（workspace scope 只为旧协议兼容）。
- 失败不抛协议错误：返回 `diagnostics`（`toMarketplaceInstallDiagnostic`）+ 空 `installedPlugins`。
- 被抑制的内置官方插件（filesystem/sea 源）走 `restoreBuiltinPlugin` 而不是写 installed record。
- 源类型：`"filesystem"|"sea"`（内置 cachePath）、字符串相对路径（市场目录内）、`{source:"directory"}`、
  `{source:"url", type:"zip", url, sha256, path?, stripRoot?, headers?}`、`github` / `git` / `git-subdir` /
  `url`（git）；`npm` / `pip` 明确不支持。
- 卸载：从 `installed_plugins.json` 移除；`removeCache` 时删安装目录与 `data/<id>`（`keepData` 保留数据）。

## 分期

1. **W1 本地源安装 + 卸载**：内置 filesystem/sea、市场目录内相对路径、`directory` 源；依赖闭包；原子激活与回滚；
   installed record；默认启用与清 suppression；`restoreBuiltin`；`uninstall`（含 removeCache / keepData）；
   per-storage 锁；`operationId` 进度通知按 TS 形状。
2. **W2 zip 源**：下载（代理/CA 与模型请求同一套）、sha256 校验、解压（防 zip-slip、`path` / `stripRoot`）。
3. **W3 git / github 源**：与 TS 一样调系统 git（重试、sparse path、ref/sha pin）。
4. **W4 市场写面**：`marketplace/add|remove|update`（CDN / url / github / 本地目录源），刷新失败落
   `lastRefreshFailure`。
5. **W5 其余**：`update`（逐条重装、诊断聚合）、`validate` / `describe`、`resolveSuggestedReference`、
   `cancelOperation`。

## 验收

每期 App 差分：同一 fixture（本地市场目录 / 本地 zip 服务 / 本地 git 仓库）下 Node 与 Rust 执行相同操作，比对协议返回、
`installed_plugins.json`（去掉时间戳）、缓存目录文件树与哈希、用户配置字节，以及随后 `plugins/list` / `overview`。

## 实现与验证（W1a：uninstall / restoreBuiltin，2026-10-02）

`crates/tools/src/plugin_uninstall.rs`：

- 选择器：`pluginId`，否则 `pluginName@marketplace`，都没有报 `pluginId or pluginName + marketplace is required`。
- 安装记录优先：`installed_plugins.json` 有该 id → 移除并整份重写 `{version:1, plugins:[...]}`（Claude 风格对象写法
  归一化成数组；数组逐条校验、记录原样保留）；默认 `removeCache` → 删安装目录与 `data/<sanitized id>`；用户配置删
  启用覆盖与选项、清掉同 id 的 suppression。
- 否则若是已发现的官方插件 → 用户配置追加 `suppressedBuiltins`（规范 id、去别名）、删启用覆盖与选项、删数据目录；
  **官方缓存保留**。返回的 `removedPlugin` 按 TS `toInstalledPluginData(record, false)` 投影。
- `restoreBuiltin`：computer-use 需 CUA 特性（否则写盘前拒绝）；清 suppression 后立即重新 seed（不受进程内
  `seed_once` 缓存影响）。
- 并发：per-storage async 锁（TS `withPluginStorageLock` 同为进程内）。

验收：`packages/services/tests/zcode-cli-rust-plugins-uninstall.test.ts`——卸载市场安装插件（缓存/数据删除、另一条
记录与其额外字段保留、用户配置清理含残留 suppression）、按 name+marketplace 卸载内置（suppression、数据删、缓存留、
list 不再出现）、未知 id、缺选择器、恢复内置（list 重新出现）；每步的协议返回、`installed_plugins.json` 与用户配置
字节、目录存在性 Node/Rust 一致。
