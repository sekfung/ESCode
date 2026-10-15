# Rust 阶段 2：`file-watcher` 域（rust-unified-host.md 子 spec，优先序 4）

> 状态：已交付（Linux 已差分；macOS/Windows 未验证，见 §6.1）· 2026-10-15
> TS 真相源：`packages/services/src/fileWatcher/{fileWatcher.ts,fileWatcherService.ts}`、`packages/shared/src/protocol.ts` 的 `FileWatchEvent`。

## 1. 范围

`ServiceChannels.FileWatcher = "file-watcher"` 的 4 个成员全部实现：`watch`、`unwatch`、`disposeAll`（方法）与 `onDynamicChange(id)`（动态事件）。
不做：`memoryDiagnostics` 的 `fileWatcher.open` 计数（同 terminal，Host 尚无诊断面）。

## 2. 所有者与状态

- Host 内唯一的 `FileWatcherService`：`id → Watcher` 表 + 自增 id（`String(nextId++)`，从 `"0"` 起，失败也消耗 id）。
  watcher 属于 Host 而不属于连接，客户端断连不回收（TS 同，注释已列为已知泄漏，不私自“修好”）。
- 底层用 `notify`（Linux inotify / macOS FSEvents / Windows ReadDirectoryChangesW），每个 Watcher 一个 notify watcher + 一个防抖 actor。
- **事件语义（照搬 TS `Emitter`）**：只投递订阅之后产生的事件；`unwatch` 取消待发的防抖事件且之后不再投递；
  `onDynamicChange` 对未知/已失效 id **不抛错**：记 warn、返回空事件流（`Event.None`）——stale id 是可恢复状态。
- 监听者通道无界（事件经 150 ms 防抖，低频小包）。

## 3. 事件生成（TS `watch` 回调 + 防抖）

1. 底层每个变更事件给出若干绝对路径。**忽略 Access 类事件**（open/close-write），其余（创建、修改、属性、删除、重命名）都算变更。
   对 Node/inotify 的实测：`create` 报 `rename`+`change`，`chmod` 报 `change`，非递归时子目录内的写不报，递归时报 `sub/n.txt`。
2. 每个变更路径加入 `pendingChangedPaths`；防抖定时器**每次变更重置**（trailing，150 ms）。
3. 到点投递一个 `FileWatchEvent`：`{dirPath: <watch 入参 path 原样>, changedPath?}`；仅当窗口内**恰好一个**已知路径且没有“未知路径”时带
   `changedPath`（`resolve(dirPath, fileName)` 的绝对路径），否则省略。
4. **被监视目录自身变化**（Node/inotify 实测）：非递归时 Node 报 `fileName=<目录自身 basename>`，`changedPath = resolve(dirPath, basename)`（一个不存在的
   “目录下同名子路径”）；递归时报空 `fileName` → 视为未知路径。Rust 对应：根自身事件按此两种映射，与 Node 全等（该路径无真实文件，不会误匹配）。
5. **错误**：底层 watcher 出错 → 发一个仅含 `{dirPath}` 的最终事件，warn 日志，清理该 watcher（TS `fsWatcher.on("error")`）。

## 4. 接口

| 方法 | 行为 |
| --- | --- |
| `watch({path, recursive?})` | 返回 `{id}`；`recursive` 缺省 `false`；失败抛 `无法监视目录 '<path>': <底层错误>` |
| `unwatch({id})` | 清理；未知 id 静默成功 |
| `disposeAll()` | 清理全部；回 `undefined` |
| `onDynamicChange(id)` | 事件流；未知 id → 空流 + warn |

底层错误文案：不存在 `ENOENT: no such file or directory, watch '<path>'`，无权限 `EACCES: permission denied, watch '<path>'`（对齐 Node 文案），其余取 notify 错误文本。

## 5. 验收

1. **TS oracle 差分**：`packages/desktop/test/rustHostFileWatcher.test.ts` 在同一组目录夹具上，用真实 TS `createFileWatcherService()` 与 Rust Host 的
   `file-watcher` channel（真实 `@escode/client`）执行同一脚本，比较每步收到的事件序列：创建单文件（带 `changedPath`）、同窗口两个文件（无 `changedPath`）、
   修改、重命名、非递归下子目录写（无事件）、递归下子目录写（带子路径）、删除被监视目录自身（非递归 / 递归两种映射）、`unwatch` 后无事件、
   stale id 订阅不抛错、`watch` 不存在目录的错误文案。
2. **Rust 单测**：防抖窗口合并、`changedPath` 取舍、根自身映射、Access 过滤。
3. **负向验证**：改坏根自身映射（非递归 / 递归各一）、放宽「恰好一个路径才带 changedPath」，差分必须失败。Access 过滤只能由单测覆盖（notify 的 inotify 掩码本身不产生 open 事件，差分里它不可见）。

## 6. 登记差异

1. **平台**：事件映射按 Linux inotify 实测校准；macOS（FSEvents）与 Windows 上 Node 与 `notify` 的事件粒度可能不同，**未在这两个平台验证**。
2. **相对路径**：Node 相对 Host 进程 cwd 解析；Rust 同样交给 `notify`（相对 Host cwd），UI 只传绝对路径，不构成差异。
3. **diagnostics**：无 `fileWatcher.open` 计数。
4. **递归实现**：Node 在 Linux 自带递归遍历；`notify` 逐目录加 inotify watch。深层新建目录里的首个事件可能先于其 watch 建立而丢失（两者都有竞态窗口，量级不同）。
5. **Host 退出**：不逐个关闭；进程退出时句柄由 OS 回收。

## 7. 交付记录

| 项 | 证据 |
| --- | --- |
| 服务实现 | `host-services/src/file_watcher/`（`mod` 服务 + 防抖 actor、`events` 事件映射）；依赖 `notify 8` |
| RPC 面 | `host-rpc/src/domains_file_watcher.rs`；`host-proto` `IMPLEMENTED` 登记 4 个成员 |
| Rust 单测 | 事件映射 3 例（Access 过滤、子路径/无路径、根自身）、服务 3 例（防抖合并与 changedPath 取舍、unwatch 取消待发事件、ENOENT 文案与 id 消耗） |
| TS 差分 | `packages/desktop/test/rustHostFileWatcher.test.ts`：创建 / 双文件 / 修改 / 重命名 / 删除 / 非递归子目录写 / 递归子目录写 / 递归 mkdir / 目录自身被删（有内容、空目录 × 非递归、递归）/ unwatch 后无事件 / 重复 unwatch / 不存在目录错误文案与 id 消耗 / stale id 订阅 / disposeAll，事件序列全等 |
| 负向验证 | ① 根自身映射改坏 → 失败；② 递归根自身不再按未知路径 → 失败；③ 多路径也带 changedPath → 失败 |
