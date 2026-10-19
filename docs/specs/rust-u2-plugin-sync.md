# Rust Host 阶段 2：plugin-sync 域（rust-unified-host.md 子 spec）

> 状态：M3 实现 · 2026-10-19
> 父计划：`rust-unified-host.md` §阶段 2；验收报告 H2.6
> TS oracle（现场核对）：
>
> - `packages/services/src/plugin-sync/pluginSyncService.ts:85-215`（7 个成员入口）、
>   `:217-269`（候选）、`:438-538`（导入）、`:907-938`（配置写入）。
> - `packages/services/src/plugin-sync/pluginSyncArchive.ts:43-123`（gzip + ustar 创建/解压）、
>   `:146-255`（目录递归/头字段）。
> - `packages/services/src/plugin-sync/pluginSyncPath.ts:7-43`（路径安全）。
> - `packages/services/src/remote-sync/remoteSyncWriteAccess.ts:1-42`（写权限探测）。
> - UI：`packages/ui/src/settings/RemotePluginSyncDialog.tsx:430-432,789-798,1007-1021,1700`。

## 1. 范围

把 `plugin-sync` channel 7 个成员从 Pending 翻为 Implemented：

| 成员                               | Renderer 调用                                |
| ---------------------------------- | -------------------------------------------- |
| `listLocalUserPluginCandidates`    | `RemotePluginSyncDialog.tsx:430`（本机服务） |
| `listRemoteUserPluginStatuses`     | `RemotePluginSyncDialog.tsx:432`（远端服务） |
| `exportPluginsArchive`             | `RemotePluginSyncDialog.tsx:789`（本机导出） |
| `importPluginsArchive`             | `RemotePluginSyncDialog.tsx:798`（远端导入） |
| `checkRemoteUserPluginWriteAccess` | `RemotePluginSyncDialog.tsx:1700`            |
| `exportMarketplaceSourceArchive`   | `RemotePluginSyncDialog.tsx:1017`（本机）    |
| `importMarketplaceSourceArchive`   | `RemotePluginSyncDialog.tsx:1027`（远端）    |

## 2. 状态所有者与文件

- Host 是用户级 plugin 配置与内联插件目录的**唯一写者**：
  - `<home>/.escode/cli/config.json`：`plugins.dirs`（绝对路径数组）与
    `plugins.enabledPlugins`（`pluginId -> boolean`）；写入保留其它键，2 空格 + 尾换行，0600。
  - `<home>/.escode/plugins/`：内联插件目标目录。
  - `<home>/.escode/plugins/marketplace-sources/`：marketplace 源镜像目录。
- 候选插件根由 `plugins.dirs` 给出；manifest 依次尝试
  `.escode-plugin/plugin.json`、`.zcode-plugin/plugin.json`、`.claude-plugin/plugin.json`、
  `.codex-plugin/plugin.json`。
- 导入是一个事务单元：目录先复制到 `<pluginRoot>/.importing-<name>-<uuid>`，rename 到目标；
  配置写失败时回滚目标目录。目录已存在或同 pluginId 已配置时 `skipped`。
- 归档是跨 Host 事实：gzip + ustar，条目路径 POSIX 相对路径；解压拒绝绝对路径、`..`、
  反斜杠、未知 tar 类型与压缩炸弹（`maxArchiveBytes`，默认 50 MiB）。

## 3. 接口

```ts
listLocalUserPluginCandidates(): { candidates: PluginSyncCandidate[]; maxArchiveBytes: 52428800 }
listRemoteUserPluginStatuses({plugins}): { statuses: PluginSyncRemoteStatus[] }
exportPluginsArchive({pluginIds}): { archive: Uint8Array; archiveBytes; plugins }
importPluginsArchive({archive, overwrite?}): { results: PluginSyncImportResultItem[] }
checkRemoteUserPluginWriteAccess(): { ok; path; error? }
exportMarketplaceSourceArchive({marketplaceId, pluginNames, source}): {
  archive; archiveBytes; marketplaceId; pluginNames
}
importMarketplaceSourceArchive({archive, overwrite?}): {
  marketplaceId; path; status: "skipped" | "synced"
}
```

错误：普通 `Error`，message 与 TS 同文（candidate not found / archive exceeds limit /
overwrite is not supported / unsafe path / manifest mismatch 等）。

## 4. 归档格式（L5）

`createPluginSyncArchive` 的字节布局：

1. 选中插件按候选顺序：目录条目（type `5`，路径尾 `/`，mode/mtime 来自 lstat），
   子项按 `localeCompare` 排序；文件条目（type `0`）按 512 字节对齐。
2. 最后写 `.escode-plugin-sync.json`：`JSON.stringify(metadata, null, 2) + "\n"`。
3. 追加 1024 字节零块，整体 gzip。

解压只接受 type `0`/`5`；文件按 header mode 恢复（Windows 跳过 chmod）；累计解压字节超过上限即失败。

## 5. 事件顺序

```
Renderer                  Host
  │ listCandidates/status   │ 读 config + manifest + 目录大小
  ├────────────────────────►│
  │◄────────────────────────┤
  │ export(local)           │ 组装 tar + metadata → gzip
  ├────────────────────────►│
  │◄────────────────────────┤ archive bytes
  │ import(remote)          │ gzip 解压 → 校验 metadata/manifest
  ├────────────────────────►│ → staging → rename → config
  │◄────────────────────────┤ results
```

本域无推送/订阅/流，两种连接路径（`/ws/host` 与 `/ws`）只要求同一请求/应答。

## 6. 已登记差异

| #   | 差异                                                                                                                                       | 原因                                                            | 移除条件                                                                                                     |
| --- | ------------------------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------ |
| 1   | `plugins.dirs` 中的相对路径按 `<home>` 解析（TS 按进程 cwd）                                                                               | Host 不读进程 cwd                                               | 配置始终写绝对路径；出现相对路径的存量数据时再评估                                                           |
| 2   | gzip 容器字节随 zlib 实现不同（Node zlib vs flate2）；解压后的 tar 逐字节一致。metadata 条目的 mtime 是导出时刻，TS/Rust 各取 `Date.now()` | 两种 zlib 的 deflate 输出/头部不同；metadata mtime 是运行时事实 | 对拍以解压后 tar 为准，仅归一 metadata 条目的 mtime+checksum；需要跨实现压缩字节一致时固定 zlib 后端与 mtime |
| 3   | 候选排序用 Host ICU collation（`collation::compare_names`）                                                                                | 与 TS `localeCompare` 同一 ICU 口径，locale 来自 Host env       | 已在 file 域登记 locale 差异，沿用                                                                           |
| 4   | `marketplace source` 的 `settings` 分支只接受内联 manifest；本地相对源按 source root 解析                                                  | 与 TS 同分支；Host 无 cwd                                       | 不计划移除                                                                                                   |
| 5   | `overwrite: true` 返回 `Error`（TS 同）                                                                                                    | 语义不支持覆盖                                                  | 不计划移除                                                                                                   |

## 7. 验收

- `plugin-sync` 7/7 Implemented；契约对账通过。
- Rust 单测：候选/manifest 回退/目录名去重/状态/skip/回滚/路径安全/解压炸弹。
- TS oracle：同一 home/workspace 夹具下，TS 与 Rust 的候选、状态、导出归档（解压后 tar）、
  导入结果全等；导入后 config 逐字节一致。
- 真实客户端：经 `@escode/client` 调 7 个成员；导入成功/跳过/坏归档失败。
- 负向验证：删除一个 Implemented 登记或方法表项，contract 测试失败。
