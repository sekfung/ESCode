# Rust 阶段 2：`commands` 域（rust-unified-host.md 子 spec）

> 状态：切片交付中 · 2026-10-17。**1/6 已交付**（`getPrimaryUserCommandsDirectory`），其余 5 个是同一域的大切片。
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

**待做部分的已知依赖**（立项时已核实，都是 Host 本地能力、无上游缺口）：
命令来源描述表（本节）、markdown frontmatter 解析/生成（`commandFileParser.ts`，165 行，自足）、
已安装插件根枚举（`installedPluginRoots.ts`，67 行，读 `<pluginStorageRoot>/installed_plugins.json`）、
四种插件清单目录（`.escode-plugin` / `.zcode-plugin`（旧）/ `.claude-plugin` / `.codex-plugin`）、
官方插件缓存根扫描、`config.json` 的路径包含校验。
**不是**它们依赖运行时或 Electron——是整个域约 1200 行 TS，宜作为独立切片整体交付。

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
