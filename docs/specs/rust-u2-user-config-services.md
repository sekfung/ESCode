# Rust Host：用户配置目录类服务（U2）

> 状态：持续追加 · 2026-10-15。这些服务都是「Host 读写用户家目录下某个约定位置」的薄 FS 服务，没有跨进程状态，
> 差分以 TS 真实服务为 oracle（`HOME` 指向临时目录）。每个服务一节：范围 / 事实源 / 登记差异 / 验收。

## 1. output-style（7 个成员）

TS 真相源：`packages/services/src/output-style/outputStyleService.ts`。

- **事实源**：`<home>/.claude/output-styles/*.md`（自定义风格）与 `<home>/.claude/settings.json` 的 `outputStyle` 键（当前激活）。`home` = Host 的家目录
  （TS：`HOME || USERPROFILE || os.homedir()`，Main 经 `rpcListen.home` 提交同一值）。
- `listStyles()`：三个内置风格（`default/explanatory/learning`，`enabled` = 是否为激活项）+ 自定义风格（`id = custom-<文件名去第一个 .md>`，`name/description`
  取自 `name:` / `description:` 行，缺 `name` 用文件名，再缺用 `Custom Style`；按 `name` 的 `localeCompare` 排序；`filePath` 是文件绝对路径）。
- `addStyle({config})`：文件名 = `name.toLowerCase()` 把连续空白换成 `-`，内容 = `name: …\ndescription: …\n\n<content>`；`updateStyle/deleteStyle` 去掉 `custom-` 前缀定位文件，
  `deleteStyle` 缺文件不报错。
- `getActiveStyle()` → `{styleId: settings.outputStyle ?? null}`；`setActiveStyle({styleId})` 读改写 `settings.json`（缺目录创建；文件损坏按空对象处理并**覆盖**——TS 同）。
- `getUserStylesDirectory()` → `{path}`（顺带创建目录）。

登记差异：

1. `settings.json` 写回时键按字典序（`serde_json` Map），TS 保持原键序；两者互读语义相同，内容缩进同为 2 空格、无结尾换行。
2. 控制台错误日志（`console.error`）不复刻，读失败按 TS 返回 null / 跳过该文件。

验收：`rustHostUserConfig.test.ts` 对真实 TS `createOutputStyleService`（`process.env.HOME` 指向临时目录）与 Rust Host 执行同一操作序列，逐步比较返回值与落盘语义。

## 2. memory（6 个成员）

TS 真相源：`packages/services/src/memory/memoryService.ts`、`projectMemoryStableRead.ts`。

- **用户级 memory**：`<home>/.claude/memory/MEMORY.md`。`loadMemory` 任何读取失败都回 `{memory:null}`，成功回 `{memory:{content, enabled:true}}`（非 UTF-8 字节按替换字符解码，同 Node）；
  `saveMemory` 建目录并写入 `config.content`；`clearMemory` 写空串，失败静默；`getUserMemoryDirectory` 建目录并回路径。
- **Project Memory（只读）**：根 = `<dataBaseDir>/.escode/cli/memories/projects`，布局 `<workspaceId>/memory/{MEMORY.md,*.md}`。
  - `listProjectMemories`：根不存在回 `[]`；根/workspace/memory 目录任一层是符号链接则跳过或报错（根本身是链接 → 报 `not a regular directory`）；只收 `MEMORY.md` 与 `*.md` 常规文件；
    文件 `index` 在前其余按 `localeCompare(_, "en")`；workspace 按 `updatedAt` 降序、同值按 id 的 `en` 排序；`label` 去掉 `-<16位十六进制>` 后缀；`updatedAt` = `mtimeMs`（libuv 公式 `sec*1000+nsec/1e6`）。
  - `readProjectMemoryFile`：片段必须是单段合法名（非空、非 `.`/`..`、无分隔符）且是 memory 文件名；逐层要求常规目录；目录项名必须与请求**逐字相同**（大小写不敏感文件系统不能别名命中）；
    打开 `O_NOFOLLOW` 句柄后再复核路径与真实路径包含关系；打开前/打开后/读完后三次比较文件身份（dev/ino/size/mtime/ctime），变化 → `PROJECT_MEMORY_FILE_CHANGED`；
    超过 5 MiB → `PROJECT_MEMORY_PREVIEW_LIMIT_EXCEEDED`（恰好 5 MiB 允许）。Node 的 fs 错误带 errno `code`（`ENOENT` 等），Rust 同样附上。

登记差异：

1. Windows 上没有 dev/ino/ctime，身份快照退化为 size+mtime，`O_NOFOLLOW` 不可用（与 TS 的 `constants.O_NOFOLLOW` 缺省 0 一致）。
2. 缺 dataBaseDir 时 Project Memory 两法回显式缺口（用户级 memory 只依赖 home，照常可用）。

验收：`rustHostUserConfig.test.ts` 的 memory 用例——用户 memory 5 步 + Project Memory 19 步（符号链接 workspace/文件、同名异大小写、目录冒充文件、越权片段、5 MiB 边界、
缺根），每步与真实 TS 服务比较结果或 `message/code`；负向验证：放宽 5 MiB 判定、放过符号链接 workspace 均失败。

## 3. broadcast（本地模式，6 个成员）

TS 真相源：`packages/services/src/broadcast/broadcastService.ts`。TS 有两种形态：带 `parentPort`（Electron host：经 Main `BroadcastHub` 跨窗口中转、claim 由 Main 仲裁）与
无 `parentPort`（web server 模式：进程内）。**本片只交付无 `parentPort` 的本地形态**；跨窗口需要 Main↔Host 控制消息（`BroadcastClaim*`/`Broadcast`），随 u2a 桌面窗口 host 进程无关化一起做。

- `send({channel, payload, sourceWindowId?})`：校验（`channel` 去空白后非空、`sourceWindowId` 为整数）后向本 Host 的**全部** `onMessage` 订阅者扇出（含发送者自己所在连接；接收端用 `sourceWindowId` 自行过滤）。
- Claim（进程内，key 去空白；空 key → `unavailable`）：`acquireClaim` → `acquired{lease:{key,token}}` / `busy{retryAfterMs = clamp(0..250, 到期时间-now)}` / `committed`；
  reservation TTL 5 s，过期在下次操作时清理；表上限 1024，超限淘汰最早插入项。`commitClaim` 把同 token 的 reservation 变永久；`releaseClaim` 释放同 token 的 reservation（迟到 token 无影响）；
  `tryClaim` = acquire + commit，仅首次为 `true`。
- 登记差异：`send` 校验失败时 TS 抛 zod JSON，Rust 抛一句话；跨窗口（`parentPort`）形态未做。
- 验收：`rustHostBroadcast.test.ts`（真实 TS 服务 `createBroadcastService(null)` 对 Rust Host：claim 状态机各分支、`tryClaim` 只首次为 true、`send` 的 trim/校验与扇出）；
  Rust 单测用注入时钟覆盖 TTL 过期与 1024 上限淘汰。

## 4. setting.ensureDefaultProject / updateDataBaseDir

TS 真相源：`settingService.ts` 的两个方法与 `paths.ts` 的 `copyDataDirectory` / `validateDataBaseDirTarget`。

- `ensureDefaultProject(userHomeDir)` → `{path: <home>/ESCodeProject, created}`（目录已存在则 `created:false`；递归创建）。
- `updateDataBaseDir(newDir?)`：目标 = `newDir.trim()` 或家目录；Windows 上目标落在应用安装目录（`ESCODE_WINDOWS_APP_INSTALL_DIR`、`%ProgramFiles%/ESCode` 等及其子目录，大小写不敏感、先规范化 `..`）
  → 抛 `DATA_BASE_DIR_FORBIDDEN_WINDOWS_INSTALL_DIR: <dir>`（带 `code`）；目标 ≠ 当前数据根则把 `<当前>/.escode/v2` 复制到 `<目标>/.escode/v2`——不覆盖已有文件、跳过 `setting.json*`（bootstrap 状态只放默认家目录）与符号链接、
  源不存在上抛 ENOENT；最后把 `dataBaseDir` 写入设置（`undefined` → 移除该键）。
- 登记差异：Windows 禁用目录判定用字符串规则复刻 `win32.normalize/resolve`，只有单测覆盖，未在 Windows 实机验证；设置写回走 Host 现有的顶层键合并（不含 TS `update` 的 schema 校验与 recentProjects 截断）；
  迁移不会让运行中的 Host 改指新数据根（TS 同样需要重启生效）。
- 验收：`rustHostSettingExtra.test.ts` 对真实 TS 设置服务（`HOME` 隔离）逐项比较返回值、目标目录树、`setting.json` 的 `dataBaseDir`；`data_dir_tests.rs` 覆盖 Windows 判定与复制规则。负向验证：复制改成覆盖已有文件即失败。
