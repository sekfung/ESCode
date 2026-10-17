# Rust 阶段 2：`commands` 域（rust-unified-host.md 子 spec）

> 状态：**6/6 已交付** · 2026-10-17（目录解析 + `list` + 写面四法）。
> TS 真相源：`packages/services/src/commands/commandsService.ts`（1007 行）、`commandFileParser.ts`（165 行）、
> `packages/services/src/plugins/installedPluginRoots.ts`（67 行）、`packages/shared/src/escode-agent-policy.ts`。

## 1. 范围

`commands` channel 的 6 个成员：

| 成员                              | 职责                                                       | 状态   |
| --------------------------------- | ---------------------------------------------------------- | ------ |
| `getPrimaryUserCommandsDirectory` | 解析「首要用户命令目录」并**确保它落盘**，回 `{path}`      | 已交付 |
| `list`                            | 多来源命令发现（用户 / 工作区 / 插件）、优先级与启用态合并 | 待做   |
| `writeCommandFile`                | 新建命令文件（markdown 片段 + frontmatter）                | 待做   |
| `updateCommandFile`               | 改写命令文件（保留未替换的 frontmatter 键）                | 待做   |
| `deleteCommandFile`               | 删命令文件                                                 | 待做   |
| `setCommandEnabled`               | 写用户 `config.json` 的 `enable` 覆盖表                    | 待做   |

**`list` 的已知依赖**（立项时已核实，都是 Host 本地能力、无上游缺口）：
命令来源描述表（本节）、markdown frontmatter 解析/生成（`commandFileParser.ts`，165 行，自足）、
已安装插件根枚举（`installedPluginRoots.ts`，67 行，读 `<pluginStorageRoot>/installed_plugins.json`）、
四种插件清单目录（`.escode-plugin` / `.zcode-plugin`（旧）/ `.claude-plugin` / `.codex-plugin`）、
官方插件缓存根扫描、`config.json` 的路径包含校验。
**不是**它们依赖运行时或 Electron——是 `list` 本身约 500 行 TS（多来源发现与优先级合并），宜单独交付。

## 2. 命令来源描述表（本片建立）

TS 的 `CommandAgentSourceDescriptor` 决定每个来源的用户/工作区目录段、扩展名、命名空间分隔符与
是否支持 `argument-hint`。当前**只有一个合法来源**（`ESCODE_COMMAND_AGENT_SOURCES` = `["escodeAgent"]`）：

| 字段                         | 值                        |
| ---------------------------- | ------------------------- |
| `agentSource`                | `escodeAgent`             |
| `directorySource`            | `escode`                  |
| `userDirectorySegments`      | `[".escode", "commands"]` |
| `workspaceDirectorySegments` | `[".escode", "commands"]` |
| `fileExtension` / `format`   | `.md` / `markdown`        |
| `namespaceSeparator`         | `/`                       |
| `supportsArgumentHint`       | `true`                    |

另有一份**目录来源**表（`COMMAND_DIRECTORY_SOURCE_DESCRIPTORS`），在描述表之上追加 `agents` 来源
（`.agents/commands`）。本片只用到用户目录，列表来源表待到 `list` 时接入。

## 3. `getPrimaryUserCommandsDirectory`

- 入参 `{agentSource?}`，缺省 `escodeAgent`；**未知来源报错**（TS 的 `COMMAND_AGENT_SOURCE_DESCRIPTORS[agentSource]`
  为 `undefined`，随后读 `.userDirectorySegments` 抛 `TypeError`）。
- 路径 = `join(home, ".escode", "commands")`。`home` 用 Host 的**设置域根**（TS `HOME || USERPROFILE || os.homedir()`；
  Main 经 `rpcListen.home` 提交同一值，见 `rust-u2-user-config-services.md` 的口径）。
- **必须 `mkdir -p`**：Windows 上文件管理器打不开不存在的路径，所以先把目录落盘再回路径（TS 注释即此因）。
- 返回 `{path}`：**原样的拼接结果**，不做 realpath（目录已存在但可能是符号链接）。

**登记差异**：无。错误文案与 TS 不逐字对齐——TS 抛的是 `TypeError`（读 `undefined` 的属性），
Rust 回一句话说明未知来源；只比接受/拒绝。

## 4. 验收

- 单测：路径拼接逐字（`join(home, ".escode", "commands")`）；目录不存在时被创建；目录已存在时幂等且不报错；
  未知 `agentSource` 报错；`agentSource` 显式传 `escodeAgent` 与缺省同结果。
- 端到端（`rpc_commands.rs`）：经真 WebSocket 调用后目录真的在磁盘上、回包路径与之一致；
  缺 `dataBaseDir`/`home` 时方法表照常登记、调用回结构化缺口（契约对账要求）。
- 负向验证：去掉 `mkdir` → 「目录被创建」用例失败。

## 5. 交付记录（`getPrimaryUserCommandsDirectory`，2026-10-17）

- **实现**：`host-services/src/commands/{mod,descriptors}.rs`（域实现 + 来源描述表）；
  `host-rpc/src/domains_commands.rs`（方法表）；`host-proto` `IMPLEMENTED` 登记 1 条。
- **测试**：`descriptors_tests.rs`（2 例：唯一合法来源与未知来源、目录拼接与描述表逐列）、
  `mod_tests.rs`（2 例：建目录与幂等、未知来源在**建目录之前**失败且不碰文件系统）、
  `tests/rpc_commands.rs`（端到端：路径落盘 + 未迁移 5 个成员的**显式缺口且列出已实现集合**）。
- **负向验证**：去掉 `mkdir` → 单测「目录必须被创建」与端到端「目录必须真的落盘」双双失败（已恢复）。
- **下一步**：`list` 是剩余 5 个里最大的一块（多来源发现 + 插件清单四格式扫描 + 启用态覆盖表合并），
  建议连 `writeCommandFile`/`updateCommandFile` 一起做——它们共用 frontmatter 的解析与生成
  （`commandFileParser.ts` 的 165 行是完整可移植单元）。

## 6. 写面四法交付记录（2026-10-17）

### 6.1 语义（逐条对照 `commandsService.ts:573-745`）

| 方法                | 关键行为                                                                                                                                                                                                                                                                                        |
| ------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `writeCommandFile`  | 目标根按 `storageLevel`（缺省 user；`project` 需要 `workspacePath`，缺失报 `Missing workspace path for project command`）→ `mkdir -p` → 同名已存在报 `Command file already exists: <文件名>` → 写文件（无既有内容，故不保留 frontmatter）→ **新建即启用**（把该路径的 `enable` 覆盖删掉）→ 回包 |
| `updateCommandFile` | 先读覆盖表与 `oldFilePath` 的既有内容（读失败当没有）→ 改名时先删旧文件、再检查新名是否已被占用 → 生成内容（保留未替换的 frontmatter 键）→ **改名且旧路径有覆盖时把覆盖迁到新路径** → 回包的 `enabled` 是**重新读一次**覆盖表（TS 同）                                                          |
| `deleteCommandFile` | `rm`，ENOENT 当作成功 → 覆盖表收敛（启用即删键）                                                                                                                                                                                                                                                |
| `setCommandEnabled` | 只改覆盖表：禁用写 `{enable: false}`，启用删键；`command` 空了连键一起删                                                                                                                                                                                                                        |

命令标识：`id = <agentSource>:<location.source>:<scope>:<name>`；
`location` = `{source: 描述表 directorySource, scope: user|project, directoryPath: 命令根, projectPath?}`；
`name` = `/` + 相对命令根、去扩展名（大小写不敏感）、按 `namespaceSeparator` 拼接的路径。

`<home>/.escode/cli/config.json` 的落盘格式 = `JSON.stringify(config, null, 2) + "\n"`；
读失败 / 坏 JSON / 非对象一律当空对象（不抛）。

### 6.2 登记差异

1. **JSON 键序**：`serde_json` 未启用 `preserve_order`，`config.json` 与回包对象按键名字典序输出，TS 保持插入序。
   两者互读语义相同（`command` 是路径 → 条目的映射，顺序无意义）。
2. `parseCommandFile` 在 TS 里包了 `try/catch` 并在失败时回 `null`；markdown 分支没有任何会抛的路径，
   因此 Rust 侧的 `Option` 恒为 `Some`——`Failed to parse written command file` 分支同样不可达（保留以便对齐）。
3. `getCrossPlatformBasename` 对 `"a/"` 这类尾随分隔符返回空串；Rust 的 `rfind` 处理同形。
4. 前端 `UserCommand` 的字段里 `description`/`argumentHint` 缺省时**整个键省略**（与 TS 的对象展开一致）。

### 6.3 验收

- 单测 14 例：解析器 5（frontmatter 拆分、无 frontmatter、多行续行、生成的两条分支、保留未替换键）、
  config 3（覆盖表增删与空桶清理、非布尔条目忽略、落盘格式与坏输入）、写面 6（新建即启用、
  project 缺 workspacePath、改名迁覆盖、改名撞名、删除幂等、启停往返）。
- 端到端 1 例（真 WebSocket）：目录解析 → 新建 → 重名报错 → 禁用 → 改名（覆盖跟随、旧键不悬空）
  → 删除（覆盖表收敛）→ `list` 仍回显式缺口且列出已实现集合。
- 负向验证两项（均已恢复）：①不迁改名的启用覆盖 → 单测「禁用状态跟随改名」失败；
  ②去掉「已存在」检查 → 端到端重名断言失败。

## 7. `list` 交付记录（2026-10-17）

### 7.1 发现顺序（可见行为，不能重排）

每个 agentSource 内：

1. **先 workspace 再 user**：先把全部 workspace 目录合并、再合并全部 user 目录。按目录交错读会让
   user `.escode` 抢在 workspace `.agents` 前面；
2. **`.escode` 是强优先级来源**：同 scope 下 `.escode` 读到**有效命令**（解析成功且文件扩展名匹配）后，
   `.agents` 不再参与；`.escode` 为空时 `.agents` 顶上；
3. **重名去重保留先出现的那条**（`dedupeCommandsByName` 的 Map 语义）——因此 workspace 胜 user；
4. 插件命令排在用户命令之后，内部按 `name` 的 `localeCompare` 排序。

回包：`{commands: 用户 + 插件, userCommands, pluginCommands, capability: {userScopeAvailable: true}}`。

### 7.2 插件来源（`commands/plugins.rs`）

候选根三类，按序合并并**按 `pluginId = <name>@<marketplace>` 去重**（先到先得）：

| 来源                                                                        | marketplace              | 缺省启用                                           |
| --------------------------------------------------------------------------- | ------------------------ | -------------------------------------------------- |
| CLI config `plugins.dirs`                                                   | `inline`                 | 是                                                 |
| 官方缓存 `<pluginStorage>/cache/zcode-plugins-official/<plugin>/<version>/` | `zcode-plugins-official` | 否（除非在 `DEFAULT_ENABLED_OFFICIAL_PLUGIN_IDS`） |
| `installed_plugins.json` 的 `installPath`                                   | 记录里的 `marketplace`   | 否                                                 |

- **清单四种目录格式**按序取第一个存在的：`.escode-plugin` / `.zcode-plugin`（旧）/ `.claude-plugin` / `.codex-plugin`；
  `name` 必须匹配 `^[a-z0-9][a-z0-9._-]{0,127}$`（trim 后），不合法即整个候选根跳过。
- 命令目录取清单 `commands`（相对路径，且**归一化后必须仍在插件根内**）；字段**缺省**时回退 `<root>/commands`。
- 插件启停：`plugins.enabled`（全局）→ `plugins.enabledPlugins[pluginId]` → 候选根的缺省启用。
- **`plugins.suppressedBuiltins` 必须显式跳过**：desktop 直接扫官方 cache，不经过 CLI resolve 的过滤，
  否则被「卸载」的内置插件仍会从 cache 贡献命令。
- 同一物理文件被多个候选根扫到时按**归一化小写绝对路径**去重。

### 7.3 登记差异

1. **`localeCompare`**：插件命令排序用 Host 注入的 ICU 比较器（`collation::compare_names`），
   与 TS 的默认 locale 排序在特殊 locale 下可能有差异（与既有 skills/tasks 的登记同口径）。
2. **目录枚举顺序**：`readdir` 顺序在两边都不保证；用户命令之间的相对顺序因此不属契约
   （插件命令有显式排序，用户命令没有——TS 同）。
3. **相对路径的 `resolve` 基准**：TS `resolve(path)` 用进程 cwd；Rust 走
   `escode_cli_core_api::env::current_dir()`（Host 不读 `std::env` 的边界约束）。两者同源。
4. **Windows 的 `isAbsolute('\\foo')`**：Node 认为绝对，Rust `Path::is_absolute` 为否——
   只影响「配置里用单反斜杠开头的相对写法」，属畸形输入。
5. JSON 键序（同 §6.2-1）。

### 7.4 验收

- 单测 9 例：`plugins_tests` 3（config 缺省与类型过滤、`resolve_inside` 的越界判定、插件名正则）+
  `list_tests` 6（递归发现与隐藏项跳过、`.escode` 遮蔽 `.agents` 及其回退、workspace 胜 user 的去重、
  禁用态透出、插件根/清单/enablement 三段、`suppressedBuiltins` 跳过并证明去掉后同一 cache 能贡献）。
- 端到端 1 例：写命令后 `list` 立即可见（名称/来源/scope/启停）、禁用后 `enabled:false`、删除后列表为空。
- 负向验证两项（均已恢复）：①去掉 `.escode` 强优先级 → 同时看到 `.agents` 的命令；
  ②去掉 `suppressedBuiltins` 跳过 → 被卸载的内置插件仍贡献命令。
