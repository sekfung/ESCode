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
无 `parentPort`（web server 模式：进程内）。**Rust Host 只有无 `parentPort` 的进程内形态，这就是最终形态**（用户决定，2026-10-17：不经 Main 中转，在进程内完成，按目标设计）。依据见下方「跨窗口」。

- `send({channel, payload, sourceWindowId?})`：校验（`channel` 去空白后非空、`sourceWindowId` 为整数）后向本 Host 的**全部** `onMessage` 订阅者扇出（含发送者自己所在连接；接收端用 `sourceWindowId` 自行过滤）。
- Claim（进程内，key 去空白；空 key → `unavailable`）：`acquireClaim` → `acquired{lease:{key,token}}` / `busy{retryAfterMs = clamp(0..250, 到期时间-now)}` / `committed`；
  reservation TTL 5 s，过期在下次操作时清理；表上限 1024，超限淘汰最早插入项。`commitClaim` 把同 token 的 reservation 变永久；`releaseClaim` 释放同 token 的 reservation（迟到 token 无影响）；
  `tryClaim` = acquire + commit，仅首次为 `true`。
- 登记差异：`send` 校验失败时 TS 抛 zod JSON，Rust 抛一句话。
- **跨窗口 = 同一 Host 的多条连接**：目标拓扑是「一个 App 一个 Host，所有窗口经 attachment 接入」（`rust-unified-host.md`），所有 Renderer 连的是同一个 `Arc<Broadcast>`，
  消息扇出（tokio broadcast，容量 256，发送者自己也收到一次）与 claim 仲裁都在 Host 进程内完成；TS 的 `parentPort` → Main `BroadcastHub` 中转是「每窗口一个 Host 进程」拓扑的产物，在这里不需要，也就不新增 `BroadcastClaim*`/`Broadcast` 控制消息。
- **不会与 TS 窗口 Host 并存**（用户确认，2026-10-17）：所以不存在需要 Main 中转的拓扑，进程内就是最终形态，不留“并存期再做中转”的后续项。若将来出现并存，必须让两边共用同一个广播域与同一个 claim 仲裁者（不能各仲裁一半），届时另立 spec。
- **跨窗口验收**：`rustHostBroadcastWindows.test.ts`——同一 Host 的 3 条连接（+1 条晚加入）：消息按序各到恰好一次（含发送者）、claim 同 key 只有一个窗口 `acquired` 其余 `busy`、错误 token 的释放无效、三窗口同时 `tryClaim` 恰好一个赢家、持有者释放后他人可获得、窗口取消订阅后其余不受影响、新窗口不收到历史消息。
  负向验证：每个连接各自一个 hub → 扇出用例失败；claim 不互斥 → 仲裁用例失败。
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

## 5. system.listIntegratedTerminalShells / probeIntranet

TS 真相源：`system/systemService.ts`、`system/integratedTerminalShells.ts`。

- `listIntegratedTerminalShells()`：非 Windows 恒为 `[]`；Windows 给 CMD（`ComSpec`，缺省 `cmd.exe`）与 Git Bash（先查 `C:\Program Files[ (x86)]\Git\bin\bash.exe`，再按 `PATH`×`PATHEXT` 找
  `git` 并推断 `..\bin\bash.exe` / `..\..\bin\bash.exe`，`source` 分别为 `system` / `path`）。环境变量走 `core_api::env`（实例快照），Windows 路径规则只有单测覆盖。
- `probeIntranet(request)`：目标规范化（tcp：host 去空白非空、端口 1–65535 否则 22、`timeoutMs` 夹到 [100,10000] 缺省 800；service：URL 必须是 http/https、`expectedMarker`/`token` 去空白）→
  每目标最多 `attempts`（1–3，缺省 2）次，目标间并发、按输入序返回；`isIntranet = 目标数>0 且 可达数 ≥ requiredSuccessCount`；`strategy` = `tcp-connect` / `service-http` / `mixed`。
  service 探测 `GET`（带 `x-zcode-intranet-token`），要求 2xx 与 JSON `{ok:true, marker?}`，`marker` 与期望不符 → `marker mismatch(expected=…, actual=…)`。
- 登记差异：①连接类错误文案由运行时给出（Node `connect ECONNREFUSED …` vs Rust `Connection refused (os error 111)`）；非 JSON 应答 TS 抛 `JSON.parse` 的 SyntaxError，
  Rust 回 `invalid service response`；②service 探测走 `escode_cli_net` 的 WebFetch 出口（代理/自定义 CA 规则生效），TS 用 Node 全局 `fetch`（不读代理配置）——内网目标应在 no_proxy 内；
  ③`http`/`https` 重定向由 reqwest 默认策略（≤10 次）处理，与 `fetch` 一致。
- 验收：`rustHostUserConfig.test.ts` 的 system 用例对真实 TS 服务（本地 TCP 与 HTTP 夹具：可达/不可达/超时、marker 符合与不符、`ok:false`、缺 `ok`、5xx、非法 URL、
  规范化与夹取、令牌头）逐项比较；`system_probe_tests.rs` 覆盖 Windows 发现规则。负向验证：把 `requiredSuccessCount` 夹取改坏即失败。

## 6. onboarding-record（9 个成员）

TS 真相源：`onboarding/onboardingRecordService.ts`、`packages/shared/src/onboardingRecord.ts`。套餐族与账号登录删除后，TS 装配的 `loadUserId` 恒为 `null`
（`node.ts`：`loadUserId: async () => null`），所以身份只剩匿名一档，`claimAnonymousRecord` 按 TS 定义是幂等空操作。

- **事实源**：`<dataBaseDir>/.escode/v2/onboarding-record.json`（v2：`{version, deviceMid, entries[], decisions[]}`；v1 读时补空 `decisions` 升到 v2）。不存在或损坏都按“从未记录”处理（损坏只告警，下次写入重建）。
- `appendRecord(deviceMid, entry)`：`deviceMid` 以文件内已有值为权威；每个用户至多一条（覆盖）；写入后清掉该用户的 decision；条目按 schema 校验并剥离未知键。
- `shouldOnboard(deviceMid)`：已有条目或 decision → `false`；否则任务索引里没有未删除任务 → `true`；有 → 记 `existing_local_user/existing_local_task` decision 并返回 `false`。
- `dismissOnboarding`：已有条目则空操作，否则写/覆盖 `dismissed/user_closed` decision。`getLatestEntry` / `syncSettingsFromRecord`（职业不在 settings 枚举 → `other`，空偏好 → `false`）/ `updateRecordPreferences` / `getRecords` / `clearRecords` 同 TS。
- 写入串行（进程内队列）+ 原子替换。
- 登记差异：①落盘键序为字典序（TS 按 schema 序），语义相同；②`hasExistingLocalTask` 在任务索引不可用（未建库/账本不符）时按“没有任务”处理（TS 会建库）；③`zod` 校验失败文案为一句话。
- 验收：`rustHostUserConfig.test.ts` 的 onboarding 用例对真实 TS 服务 21 步（缺省读取、dismiss、append 覆盖、未知职业回填、偏好更新、非法入参、clear）逐步比较返回值与落盘文件；`onboarding_tests.rs` 覆盖 ISO 时间、
  已有本地任务决策、v1 升级与损坏文件。负向验证：append 不清 decision 即失败。
