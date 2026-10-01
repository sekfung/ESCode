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

## 实现与验证（W1b：本地源安装，2026-10-02）

- `crates/tools/src/atomic_dir.rs`：与 TS `atomic-directory.ts` 同布局的目录原子激活（同目录暂存 → 排他写 v2 事务标记
  `.<name>.transaction.json`（owner pid / ownerId / transactionId / authorityPath）→ 旧目标改名 `.<name>.backup` → 暂存改名为
  目标），`finalize` 删备份与标记、`rollback` 还原备份；`recover` 按 TS `recoverAtomicTargetSync` 规则收拾任一 runtime 崩溃
  留下的半成品（存活写者不动；权威状态含同一 `cacheTransactionId` 视为已提交）。进程存活判断复用 host 的 Node 语义
  `process_alive`。
- `crates/tools/src/plugin_install.rs`：依赖闭包（后序、去重、环检测、跨市场白名单 `allowCrossMarketplaceDependenciesOn`、
  `name@^x` 约束后缀剥离）→ 解析源根（内置 filesystem/sea 的 cachePath、市场目录内相对路径（含 `metadata.pluginRoot`）、
  `directory`、无 source 时按名字目录）→ 版本取源根 plugin.json 的非空 version，否则条目 version，否则 0.0.0 → 原子激活到
  `cache/<market>/<name>/<version>`（源即目标时不拷贝）→ `strict:false` 无 manifest 时合成 `.claude-plugin/plugin.json`
  （剔除来源/商店展示字段）→ 重写 `installed_plugins.json`（已有记录原地覆盖、保留首次 installedAt、带 cacheTransactionId）；
  任一步失败逆序回滚。之后官方市场清 suppression、默认启用只写尚未声明的 id。错误按 TS `toMarketplaceInstallDiagnostic`
  归类为诊断返回（不抛协议错误）。被抑制的内置官方插件走 restore。
- 远端源（github / git / url / git-subdir）在 W2/W3 前返回 `plugin_marketplace_source_unsupported`；`dryRun` 校验与
  市场按需刷新（manifest 不在本地时）尚未支持，明确报错而不是伪造结果。

验收：`packages/services/tests/zcode-cli-rust-plugins-install.test.ts`——本地市场：相对路径插件依赖 directory 源插件
（`helper@^1.0`）、用户配置显式停用依赖、重装（installedAt 保留）、`strict:false` 合成 manifest、依赖环 / 跨市场 /
依赖缺失 / 非法 source kind / 未知插件五种诊断；协议返回、`installed_plugins.json`、缓存文件树与内容哈希、用户配置字节、
合成 manifest 与随后 `plugins/list`，Node 与 Rust 逐值一致。

## 实现与验证（W2：zip 源，2026-10-02）

`crates/tools/src/plugin_zip.rs`（新增依赖 `zip`，仅 deflate），对齐 TS `zip-source.ts`：

- 字段读取顺序同 TS（url → headers → path → sha256 → stripRoot），文案逐字；URL 只允许 HTTPS，回环（localhost /
  127.x / ::1）允许 HTTP；禁用 `authorization` / `cookie` / `proxy-authorization` / `set-cookie` 头；sha256 必须 64 位 hex。
- 下载：与 WebFetch 共用 `web_fetch::proxied_client`（同一套代理解析与设置页 CA，不自动跟随重定向）；手动重定向最多 5 次，
  跨源丢弃自定义头；200 MiB 上限、180 s 超时；sha256 不符报 `expected=…, actual=…`。
- 解压：路径规范化（拒绝空段 / `.` / `..` / 反斜杠 / 绝对路径 / 盘符）、拒绝加密、符号链接与非常规类型条目；条目数 ≤ 20000、
  单文件 ≤ 50 MiB、总量 ≤ 500 MiB。
- 根定位：显式 `path` > 解压根已有 manifest > `stripRoot`（缺省 true）且只有一个顶层目录 > 解压根；安装前按 TS
  `assertZipPluginInstallRoot` 要求根能形成合法插件且 manifest 名与条目一致；临时目录在激活后清理。

验收：`packages/services/tests/zcode-cli-rust-plugins-install-zip.test.ts`——回环 HTTP 服务（yazl 生成 zip）：单顶层 strip、
显式 path、同源重定向转发自定义头、sha256 不符、manifest 名不符、多顶层无 manifest、非 HTTPS、禁用头；协议返回（下载类
错误只比 code）、`installed_plugins.json`、缓存文件树与服务端收到的请求（路径与自定义头），Node 与 Rust 一致。

## 实现与验证（W3：仓库类源，2026-10-02）

`crates/tools/src/plugin_git.rs`，对齐 TS `resolveRepositoryPluginSource` / `github-archive-source.ts` / `clonePluginSource`：

- 分流（`plugin_install::repository_source`）：`github`（`repo` → `https://github.com/<repo>.git`）、`git`、`url`（无 type 或
  `git`）、`git-subdir`（`owner/repo` 简写补 GitHub HTTPS）；必填字段文案逐字；pin 取 `sha`，其次旧写法 `commit`。
- 公开 GitHub HTTPS 仓库先走 Archive：`api.github.com/repos/<o>/<r>/zipball/<sha ?? ref ?? HEAD>`，复用 W2 下载解压（要求单顶层、
  不校验 sha）；submodule / Git LFS（含父目录继承的 `.gitattributes`）需要完整 Git 语义时回退。回退条件同 TS：非公开 GitHub、
  401/403/404、符号链接/特殊条目；其余 Archive 失败报 `plugin_archive_fetch_failed`（URL 凭据脱敏）。
- 系统 Git：`ZCODE_GIT_BINARY` 可覆盖；子进程环境走 host `child_env::apply(…, true)`（清洗后恢复出网配置，同 TS
  `buildMarketplaceGitEnv`）；无 sha 时 `--depth 1`、`--branch <ref>`、sha 时 clone 后 `checkout`；网络型错误最多 3 次、
  间隔 1 s × 次数；90 s 超时；git 不存在报 `plugin_git_unavailable`。
- `zip` 模块抽出 `resolve_http`（可选 sha、`require_single_root`）与带 HTTP 状态的 `ZipDownloadError`。

验收：`packages/services/tests/zcode-cli-rust-plugins-install-git.test.ts`——测试内建本地仓库（两次提交 + `v2` 分支 + 子包）：
普通 clone、ref、sha pin（检出旧提交）、git-subdir、子目录缺失、仓库不存在、缺 url；以及 `ZCODE_GIT_BINARY` 指向不存在路径时的
Git 不可用诊断。协议返回（git 子进程原文只比 code）、安装记录与缓存文件树 Node/Rust 一致。GitHub Archive 主链路需要访问
api.github.com，未在离线差分里覆盖（回退判定与 URL 解析有单测）。
