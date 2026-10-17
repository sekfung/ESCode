# Rust 阶段 2：sync 三域（rust-unified-host.md 子 spec）

> 状态：已收口 · 2026-10-17。三个域共 19 个成员全部交付：
> `skill-sync` 5/5、`mcp-sync` 8/8、`settings-sync` 6/6。按域分三次提交。
> 共同点：都是 **Host 级服务**（TS 在 local 与 remote 两侧都注册），用户根是 `rpcListen.home`
> （homedir 语义，经 `channels_with(home, …)` 注入），不依赖工作区或 agent 实例。
> 远端同步的方向：本地 Host 导出 → 文件/网络传输 → 远端 Host 导入；归档与配置文件是两端共享的事实。

## 1. skill-sync（5/5）

TS 真相源：`skill-sync/skillSyncService.ts` 509、`skillSyncArchive.ts` 346、`skillSyncPath.ts` 43、
`skillSyncErrors.ts` 23、`skillSync.ts` 30。

### 1.1 接口

| 方法                                       | 行为                                                                           |
| ------------------------------------------ | ------------------------------------------------------------------------------ |
| `listLocalUserSkillCandidates()`           | 扫 `.escode/skills` + `.agents/skills`，返回候选与 `maxArchiveBytes`（20 MiB） |
| `listRemoteUserSkillStatuses({…})`         | 按目录名判断远端是否已存在；支持按 skill `name` 回退匹配已有目录               |
| `exportSkillsArchive({skillIds})`          | 按候选 id 选目录，生成 gzip+tar 归档；字节数上限 20 MiB                        |
| `checkRemoteUserSkillWriteAccess()`        | 在 `.escode/skills` 写入并删除探测文件，返回 `{ok,path,error?}`                |
| `importSkillsArchive({archive,overwrite})` | 解压到临时目录，按目录名与 skill 名双重判重后复制；返回逐项结果                |

错误形状：普通错误 `name = "Error"`；超限错误 `name = "SkillSyncSizeLimitError"`、
`code = "SKILL_SYNC_SIZE_LIMIT_EXCEEDED"`、`data = {actualBytes,maxBytes,phase}`（UI 按 phase 本地化）。

### 1.2 候选扫描

- 复用 `skills` 域的有界遍历策略（`should_walk`、`MAX_SCAN_DEPTH`、`SKILL.md`），
  不另写递归；扫描错误静默跳过（TS 不传 `onError`）。
- 每个 `SKILL.md` 的目录成为一个候选：
  - `directoryName` = 相对根的路径（`/` 分隔）；根目录自身的 SKILL.md 不算候选。
  - `name` = frontmatter `name.trim()`，缺失时回退为**相对路径最后一段**。
  - `description` = frontmatter `description.trim()`，缺失为空串。
  - `id` = `sha256(绝对目录路径)` 的十六进制（64 字符）。
  - `path` = `SKILL.md` 绝对路径。
  - `sizeBytes` = 递归体积：软链跟随目标（文件取目标 size、目录按目标递归），
    目录按 realpath 去重（环返回 0）；不跳过 `node_modules` 等目录（体积是真实占用）。
- **去重顺序**：
  1. `.escode` 候选按 `directoryName` 排序后，按 `SKILL.md` 的 canonical path 去重（realpath 失败回退原路径）；
  2. `.agents` 候选先排除两类：目录名属于 `.escode` 下**任何**含 `SKILL.md` 的目录（含非候选路径），
     frontmatter name（trim+lowercase）与 `.escode` 候选重名；
  3. 再与 `.escode` 共用 canonical path 去重；
  4. 合并后按 `directoryName` 以 Node `localeCompare`（复用 `collation::compare_names`）排序。

### 1.3 远端状态

- 根固定为 `.escode/skills`；`directoryNames` 逐个做安全相对路径归一（拒绝绝对路径、`..`、
  反斜杠、盘符、空段）。
- 存在（`existsSync`）→ `{directoryName, exists:true, path}`。
- 不存在时，若请求的 `skills` 里有同名条目，用其 `name` 在**用户级两个根**（`.escode` 先、`.agents` 后）
  按 skill 名找已有目录；找到 → `exists:true` + 已有路径；否则 `exists:false`。
- skill 名键 = `trim().toLowerCase()`；同名以遍历中先出现者为准。

### 1.4 归档格式

- **gzip + ustar tar**，只写目录（type `5`，路径带尾 `/`）与普通文件（type `0`）：
  - 目录内容按文件名 `localeCompare` 排序后递归；符号链接/特殊文件 → `unsupported skill archive source: …`。
  - 头字段与 TS 相同：mode（`st_mode & 0o777`）、uid/gid 0、size、mtime 秒、`ustar`/`00`、
    uname/gname `escode`；路径 >100 字节时按段拆 `prefix`（≤155）+ `name`（≤100），否则报路径过长。
  - 末尾 1024 字节全零块。
- **读取**：流式解压（不一次性展开，避免压缩炸弹占内存）；只接受 type `0`/`\0`/`5`；
  文件累计字节超过 `maxExtractedBytes` → `extracted-content` 超限；路径逐个做安全归一并限制在目标根内；
  截断 → `truncated skill sync archive`。
- 上限 20 MiB 在三个阶段分别校验：`selected-content`（候选体积和）、`archive`（gzip 字节数）、
  `extracted-content`（解压文件累计）。

### 1.5 导入语义

- `overwrite` 为真 → `Error("skill sync overwrite is not supported")`。
- 先解压到 `<root>/.sync-tmp-<uuid>`，结束后无论如何删除临时目录。
- 提取目录按 `directoryName` 排序；逐项：
  1. 目标已存在 → `skipped` + 目标路径；
  2. 已有同名 skill（用户级两个根）→ `skipped` + 已有路径；
  3. 否则递归复制（文件 `create_new`，已存在即失败；目录合并；软链按链接复制）→
     `synced` + 目标路径，并把名字写入本次的已存在表；
  4. 复制失败 → `failed` + `error`。
- `SKILL.md` 缺失的项（收集后竞态）→ `failed` + `error = "SKILL.md is missing"`。

### 1.6 写权限探测

- `mkdir -p <root>` 后以独占创建写 `.escode-sync-preflight-<pid>-<uuid>`（内容 `ok`）再删除。
- 失败返回 `{ok:false,path,error}`，不抛错；清理失败忽略。

### 1.7 登记差异

1. 目录遍历的并列顺序：`.escode` 内同名 skill 的「先出现者」按路径有序遍历（Rust BTreeSet）
   vs TS 的 DFS 产出顺序；只在同一根内出现重名时可见。
2. 错误文案：文件系统错误用 Rust `io::Error` 文本（TS 是 Node 的 `code: message, syscall 'path'`）；
   稳定 `code`（超限）不受影响。
3. gzip 头（OS/压缩实现）字节不同；tar 负载与 TS 读取器互操作。

### 1.8 验收

- 单测：路径归一/越界拒绝；归档往返（目录+文件、排序、mode/mtime、路径拆分）、恶意路径拒绝、
  截断与不支持类型、解压上限；候选扫描（两级根、目录名/名字去重、realpath 去重、嵌套 skill、
  体积与软链）、远端状态（直接存在、按名字回退、不存在）；导入（synced/skipped/失败、临时目录清理）；
  写权限探测。
- 端到端：临时 home 起服务，`listLocalUserSkillCandidates` 返回夹具；A 导出 → B 导入 → B 再扫到
  同名候选，二次导入全部 skipped；`checkRemoteUserSkillWriteAccess` 为 ok。
- 负向验证：①去掉 `.agents` 的名字过滤 → 重名用例失败；②去掉解压路径归一 → 穿越归档用例失败。

### 1.9 skill-sync 交付记录（2026-10-17）

- **实现**：`host-services/src/skill_sync/`
  - `candidates.rs`：两级根扫描（复用 `skills` 域的 `walk_markdown_paths` / `policy`）、
    `.escode` 目录名与 skill 名双重过滤、realpath 去重、递归体积（软链跟随 + 环去重）、
    TS 口径的 frontmatter 解析（saphyr，失败回退目录名）；
  - `paths.rs`：安全相对路径归一与根内解析；
  - `archive.rs` + `archive_tar.rs`：gzip + 手写 ustar（type `0`/`5`、mode/mtime/prefix/checksum），
    流式解压、`parseInt(_, 8)` 口径的八进制解析、三阶段上限；
  - `import.rs`：临时目录解压 → 目录名/name 双重判重 → `synced/skipped/failed`；
  - `service.rs`：五个成员 + 写权限探测。
- **通道**：`domains_skill_sync.rs`（`skill-sync`，Host 级，home 经 `channels_with` 注入）；
  `inventory_services.rs` 补 5 条。
- **共享件**：`host-rpc/src/wire_bytes.rs` 集中 `@escode/rpc` 的嵌套 Uint8Array 编解码，
  `serial.write` 的 `param_bytes` 改为复用同一实现；`skills` 域把遍历与扫描策略提为 crate 可见。
- **测试**：新增 17 例（路径 2、tar 头 3、归档 5、候选 3、服务 5——含导入/跳过/三阶段超限/写权限）
  - 端到端 1 例（A 导出 → B 导入 → 二次 skipped；未知 id 与不安全路径的错误形状）。
- **负向验证**（两项，均已恢复）：①去掉 `.agents` 的 skill 名过滤 → 去重用例失败；
  ②去掉解压路径归一与根内限制 → 穿越归档用例失败（并确认文件真的落到了目标根之外）。
- **门禁**：workspace 全部测试目标通过（host-services 434、host-rpc 119 及全部 e2e）；
  clippy `-D warnings` 0；`pnpm check:escode-host` 通过；库存 **278 已实现 / 260 待办**。

## 2. mcp-sync（8/8）

TS 真相源：`mcp-sync/mcpSyncService.ts` 937、`mcpSync.ts` 47；共享类型 `packages/shared/src/mcp-sync.ts`、
`mcp.ts`、`settings-source.ts`。

### 2.1 接口

| 方法                                         | 行为                                                                            |
| -------------------------------------------- | ------------------------------------------------------------------------------- |
| `loadMcpFromUserDirectory({workspacePath?})` | 依次读工作区（`.escode`→`.agents`）与用户级目录，返回 `NativeMcpServerRecord[]` |
| `listWorkspaceMcpServerStatuses(params)`     | 转发 workspace 实例的 `mcp/list`（运行态连接检查）                              |
| `saveMcpToUserDirectory(payload)`            | `upsert` / `delete` / `set-enabled` 写 `.escode` 配置                           |
| `listLocalUserMcpCandidates()`               | 有效用户记录（`.escode` 非空优先，否则 `.agents`）→ 候选 + `localHomeDir`       |
| `listRemoteUserMcpStatuses({names})`         | 按名字（trim+lower）判断远端是否存在，返回路径                                  |
| `exportMcpServers({serverIds})`              | 按候选 id 导出 `{id,name,config,enabled,source,path}`                           |
| `checkRemoteUserMcpWriteAccess()`            | 在 `<home>/.escode/cli` 做写权限探测                                            |
| `importMcpServers(params)`                   | 写用户级 `mcp.servers`，按目标文件/用户级同名双重判重，改写 filesystem 参数     |

### 2.2 目录与键

| 来源     | 用户级                           | 工作区级                          | server 键     |
| -------- | -------------------------------- | --------------------------------- | ------------- |
| `escode` | `<home>/.escode/cli/config.json` | `<workspace>/.escode/config.json` | `mcp.servers` |
| `agents` | `<home>/.agents/mcp.json`        | `<workspace>/.agents/mcp.json`    | `mcpServers`  |

- 读面「有效记录」：`.escode` 文件里有任何 server 就用它，否则整体回退 `.agents`（不是逐条合并）。
- `enabled` 只认显式 `false`（`config.enabled !== false`）；写面启用态不落冗余字段，
  停用写 `enabled: false`，并同时清掉历史 `enable`。

### 2.3 存量 `enable` 迁移

- 只在 server 配置里出现 `enable` 时改写；`enable:false` 或 `enabled:false` 任一为假即停用
  （冲突以停用为准）。迁移结果就地落盘；写盘失败不阻断读取（内存结果已是新口径）。

### 2.4 写面

- `upsert`：`config` 缺失 → `Missing MCP config for upsert action`；与已有 server 合并后写回，
  保留 `mcp` 的其它字段与文件其它顶层字段。
- `delete`：删除该 server（未知 action 与 delete 同路径，TS 的 `else`）。
- `set-enabled`：缺 `enabled` 布尔值 → `Missing enabled value for MCP set-enabled action`；
  `location` 缺省时用 `.escode` + `projectPath ? workspace : user`。server 不存在时不写文件。
- 写盘：`JSON.stringify(_, null, 2) + "\n"`，原子写 + 临时文件 `0600`（配置含 secret）。

### 2.5 导入与 filesystem 路径改写

- `overwrite` 为真 → `Error("mcp sync overwrite is not supported")`。
- 判重：目标文件已有同名 → `skipped` + 目标路径；用户级有效记录已有同名（trim+lower）→
  `skipped` + 已有路径。
- 写入前 `setServerEnabled(config, server.enabled)`，再对 **stdio filesystem** server 改写 `args`：
  - 名字是 `filesystem`/`file-system`/`fs`，或 command/args 里出现
    `@modelcontextprotocol/server-filesystem` / `mcp-server-filesystem`；
  - 参数落在 `localWorkspacePath` 内且 `remoteWorkspacePath` 非空 → 改写到远端工作区；
    否则落在 `localHomeDir` 内 → 改写到 `remoteHomeDir`；其余原样；
  - 远端根的形态（`/...`、`X:\...`、`\\server\share`）决定 posix/win32 拼接，不看本机平台。
- `config` 不是对象 → 该项 `failed`（TS 在 clone/setServerEnabled 处抛错）。

### 2.6 运行态状态

- `listWorkspaceMcpServerStatuses` 经 `forward::runtime_target` 建 workspace 引用，转发 `mcp/list`
  （`mcpServers`/`mode` 原样带上），结果 `{statuses}`；错误按运行时协议错误透传
  （`ESCodeProtocolClientError`）。

### 2.7 登记差异

1. **没有独立的 mcp-status 进程 lane**：TS 用专用 agent 进程避免慢握手堵住会话队列；Rust 复用 workspace
   实例，慢 `mcp/list` 会与会话请求共享队列。可见条件：连接大量 MCP server 且会话繁忙时。
2. **没有旧 Agent 的 `mode`/`mcpServers` 兼容重试**：Rust CLI 运行时支持当前协议；只有混跑旧 TS Agent
   时才需要，Host 与运行时同版本。
3. JSON 键序按 serde_json 的字母序（TS 保留插入序）；文件语义一致。
4. 文件系统错误文本用 Rust `io::Error`；读取/解析/非对象的错误文案保持 TS 中文原文。
5. `mcp/list` 超时用 Host 统一的 30s（TS 默认协议超时同值）。

### 2.8 验收

- 单测：路径/位置；JSON 读写与错误文案、0600、`mcp` 其它字段保留；`enabled`/`enable` 迁移；
  filesystem 参数改写（posix/Windows/远端 Windows/根路径）；load 的 workspace→user 顺序与来源回退；
  save 的 upsert/delete/set-enabled 与 legacy 清理；候选/状态/导出；导入判重与改写。
- 端到端：A 导出 → B 导入（参数改写）→ 二次 skipped；save upsert 保留其它字段；状态查询与写权限；
  `mcp/list` 转发返回 `{statuses}`。
- 负向验证：①去掉导入的用户级同名判重 → `.agents` 同名用例失败；
  ②跳过 filesystem 改写 → 路径改写用例失败。

### 2.9 mcp-sync 交付记录（2026-10-17）

- **实现**：`host-services/src/mcp_sync/{paths,config,rewrite,read,import,service}.rs`；
  配置写入走 `escode_host_core::atomic_write::write_secret`（新增，0600）；
  写权限探测抽到 `host-services/src/remote_sync.rs`，与 skill-sync 共用。
- **通道**：`domains_mcp_sync.rs`（`mcp-sync`）；`listWorkspaceMcpServerStatuses` 在通道内转发
  `mcp/list`（StartIfNeeded）；`inventory_services.rs` 补 8 条。
- **测试**：新增 18 例（路径 3、配置 5、改写 4、读取/保存 3、服务 3）+ 端到端 1 例
  （含真实运行时的 `mcp/list` 转发）。
- **负向验证**（两项，均已恢复）：见 §2.8。
- **门禁**：workspace 全部测试目标通过（host-services 454、host-rpc 119+ 及全部 e2e）；
  clippy `-D warnings` 0；`pnpm check:escode-host` 通过；库存 **286 已实现 / 252 待办**。

## 3. settings-sync（6/6）

TS 真相源：`settings-sync/settingsSyncService.ts` 2377、`settingsSync.ts` 41、`types.ts` 33；
共享类型 `packages/shared/src/settings-sync.ts`。

### 3.1 接口与分片

| 方法                                          | 状态   | 行为                                                            |
| --------------------------------------------- | ------ | --------------------------------------------------------------- |
| `getClaudeAgentsFileMigrationStatus({…})`     | 已交付 | `~/.claude/CLAUDE.md` → `~/.escode/AGENTS.md` 的存在性/可用性   |
| `copyClaudeAgentsFileToEscodeAgentsFile({…})` | 已交付 | 复制（`overwrite === true` 才覆盖）；返回 copied/skipped 与原因 |
| `getFirstRunPromptState()`                    | 已交付 | 设置文件顶层键 `settingsSyncFirstRunPromptHandled === true`     |
| `markFirstRunPromptHandled()`                 | 已交付 | 顶层合并写 `settingsSyncFirstRunPromptHandled: true`            |
| `detect({categories,intent})`                 | 已交付 | 多 agent 设置发现（skills/commands/plugins/mcpServers）         |
| `importSelected({selections})`                | 已交付 | 按选择导入（copy/symlink，含 CLI 配置合并）                     |

- 前 4 个方法不使用 `workspacePath`/`workspaceIdentity`（TS 同样 `void` 掉）；
  `detect`/`importSelected` 用 `workspacePath` 解析工作区源根与目标根。

### 3.2 AGENTS.md 迁移

- 存在性用 `lstat`：软链本身算存在（含悬空软链）；目录也算存在。
- 源缺失 → `{status:"skipped", skippedReason:"missingSource"}`；状态里 `supported:false` +
  `unavailableReason:"missingSource"`。
- 目标存在且 `overwrite !== true` → `skipped` + `targetExists`；只有严格布尔 `true` 才覆盖
  （字符串/数字真值不覆盖）。
- 复制前 `mkdir -p` 目标父目录；`copy` 跟随源软链、覆盖目标；`overwritten` = 复制前目标是否存在。

### 3.3 首启提示状态

- 读：设置服务 `read(home)` 后取 `settingsSyncFirstRunPromptHandled`，严格等于 `true` 才算 handled。
- 写：设置服务顶层合并补丁（保留其它键、原子写、2 空格 JSON + 换行），不清除导入结果。

### 3.4 `detect` / `importSelected` 设计

TS 的入口只有两种组合：`detect` 仅在 `intent === "manualImport"` 且 `categories` 包含四类之一时
扫描**该类**（顺序 skills → commands → plugins → mcpServers），否则返回 `{agents: []}`；
`importSelected` 按 `selections` 顺序逐项导入并汇总计数。

- **路径表**：skills/commands/plugins 各 16 个 agent 的 project/global 根（`.claude`、`.codex`、
  `.opencode`/`.config/opencode`、`.openclaw`/`skills|commands|plugins`、`.augment`、`.continue`、
  `.goose`/`.config/goose`、`.qwen`、`.qoder`、`.qoder-cn`、`.windsurf`/`.codeium/windsurf`、`.trae`、
  `.trae-cn`（仅 skills）、`.kiro`、`.roo`、`.codebuddy`）；MCP 12 个来源（含 `agents`）与 3 种阅读格式
  `mcpServersJson` / `mcpJson` / `codexToml`（TOML 用 `toml` crate）。
- **发现顺序**：所有 user 根在前、workspace 根在后；同一源路径只取第一次出现（`trae`/`traeCn`
  的项目根相同 → 归 `trae`）。`sourceRootScope`：workspace 根 → `project`，user 根 → `global`。
- **目标根**：user → `<home>/.escode/{skills,commands,plugins,cli/config.json}`；
  workspace → `<workspace>/.escode/{skills,commands,plugins,config.json}`。缺 workspacePath 时
  workspace 目标为 `null`（导入按 `skipped` 处理）。
- **候选**：
  - skills：有界遍历（复用 `skills` 策略），`sourcePath` 是技能目录，目标 = 目标根/目录 basename；
    名称/版本按 TS 的局部 YAML 解析（含 loose `name:`/`version:` 回退），无可读文件时回退目录名。
  - commands：递归扫描非点目录的 `.md`（无深度上限），名称 = `/相对路径去掉 .md`；
    `description`/`argument-hint` 复用 Rust 的命令 frontmatter 解析器。
  - plugins：根下一层目录/软链，按 `.escode-plugin` → `.zcode-plugin` → `.claude-plugin` →
    `.codex-plugin` 找 `plugin.json`；`id = name@inline`；无合法 name 的目录跳过。
  - mcpServers：文件存在性用 `lstat`；Claude 的项目源有 `.claude/settings.json` 与 `.mcp.json` 两个；
    OpenCode `mcp` 表的 `command` 数组折叠为 `command`+`args`、`local/remote` → `stdio/http`；
    Codex 读 `mcp_servers`/`mcpServers`；导入前删除 `timeout`/`startup_timeout_sec`；
    `sourcePath = <文件>#<name>`。
- **判重**：
  - skills：目标目录 `readdir` 成功即 `targetExists`；目标根内已有同名 skill → `sameNameExists`。
  - commands：目标文件 `lstat` 存在即 `targetExists`；目标根内同名相对路径 → `sameNameExists`。
  - plugins：目标目录存在 → `targetExists`；目标的 `plugins.dirs` 里已配置同 id（小写）→ `sameNameExists`。
  - mcpServers：目标 `.escode` 配置 `mcp.servers` 已有同名 → `sameNameExists`（无 targetExists）。
  - 判重缓存在一次汇总/一次导入内共享（同一目标根/配置只读一次）。
- **发现结果**：只返回 `discovered`（候选数 > 0）的 agent，按路径表顺序；每项含一个 category：
  `discoveredCount`/`importableCount`/`skippedCount`/`sourcePaths`（去重排序）/`sourceRoots`
  （按 scope→path 排序，含逐项 importable/skipReason）；skills 的 `selectedByDefault = importable > 0`，
  其余 false。
- **导入**：
  - `importMode` 默认 `symlink`：skills/plugins 建目录软链（Windows 用目录软链，登记与 TS junction 的差异）；
    commands 在 Unix 建文件软链、Windows 先硬链失败回退复制；`copy` 用 `cp -R` 等价语义
    （目标存在即失败）。
  - skills/commands 的目标路径在选中 `targetScope` 下重新解析；plugins/mcp 的 `targetScope`
    缺省取候选的 `sourceRootScope`。
  - plugins 成功后把**解析后的目标路径**追加到目标配置 `plugins.dirs`（已存在则不写）。
  - mcpServers 合并到目标配置 `mcp.servers`，保留其它字段；写盘走原子写（TS 是直接写，登记差异）。
  - 每个候选独立 try/catch → `failed`；任务状态：有 failed → `failed`，否则 imported>0 → `success`，
    否则 `skipped`；顶层计数为各任务之和。

### 3.5 登记差异

1. `copyClaudeAgentsFileToEscodeAgentsFile` 不写 TS 的 info 日志（Host 无日志设施）。
2. 首启状态的读取沿用 Rust 设置服务的坏文件隔离/回退（与 TS settingService 同语义）。
3. Windows 的目录软链用 `symlink_dir`（可能需要开发者模式），TS 用免提权的 junction。
4. 导入写配置文件走原子写（临时文件 + rename）且权限 0600；TS 是直接写。
5. JSON 键序按 serde_json 的字母序（TS 保留插入序）；文件语义一致。
6. 命令元数据的解析复用 `commands` 域的 frontmatter 解析器（与 TS `CommandFileParser` 同口径）；
   skill 名称/版本用本地 YAML + loose 回退，与 TS 的 `yaml` 解析在极端语法上可能有差异
   （未加引号的 `: `、重复键等）。

### 3.6 settings-sync 交付记录（2026-10-17）

- **实现**：`host-services/src/settings_sync/`
  - `agents.rs` + `sources.rs`：18 个 agent 枚举、四类来源路径表（16/16/15/12 项）与 user/workspace 根展开；
  - `metadata.rs`：skill 名称/版本（YAML + loose 回退）、命令元数据（复用 commands 解析器）、插件 manifest；
  - `mcp_source.rs`：`mcpServersJson` / OpenCode `mcpJson` / Codex TOML 读取与归一、timeout 剥离；
  - `candidates.rs`：四类候选与源路径去重；
  - `discovery/{mod,skip,summary}.rs`：`detect`、判重缓存、`sourceRoots` 汇总；
  - `import/{mod,skills_commands,plugins_mcp}.rs`：`importSelected`、copy/symlink 两种模式、
    命令文件软链/硬链回退、插件配置 `plugins.dirs`、MCP 配置 `mcp.servers` 合并；
  - `agents_file.rs` + `first_run.rs`：AGENTS.md 迁移与首启状态。
- **通道**：`domains_settings_sync.rs` 六个方法全部登记；`inventory_services.rs` 补 2 条
  （`detect` / `importSelected`）。
- **测试**：24 例（元数据 4、MCP 读取 4、候选 3、发现 4、导入 5、AGENTS 3、首启 1）
  - 端到端 1 例（迁移/首启 + `detect → importSelected → detect` 闭环）。
- **负向验证**（两项，均已恢复）：①忽略 `targetScope` → 项目作用域导入用例失败；
  ②不剥离外部 `timeout` → 配置合并用例失败。
- **门禁**：workspace 全部测试目标通过；clippy `-D warnings` 0；`pnpm check:escode-host` 通过；
  库存 **292 已实现 / 246 待办**（sync 三域 19/19）。
