# Rust 阶段 2：`subagents` 域（rust-unified-host.md 子 spec）

> 状态：切片交付中 · 2026-10-17。**4/8 已交付**（目录解析 + 状态写入面）。
> TS 真相源：`packages/services/src/subagents/`（`subagentsService.ts` 892、`subagentMarkdown.ts` 435、
> `subagentConfigScope.ts` 343、`subagentRuntimeConfig.ts` 230、`subagentStorage.ts` 65）、
> `packages/shared/src/subagents-types.ts`。

## 1. 范围与切片

| 成员                                          | 职责                                                 | 状态   |
| --------------------------------------------- | ---------------------------------------------------- | ------ |
| `getPrimaryUserAgentsDirectory`               | 解析用户 agent 根并确保落盘                          | 已交付 |
| `setEnabled`                                  | `agents-state.json` 的 `disabledAgentIds`            | 已交付 |
| `setBuiltInModelOverride`                     | `builtInModelSelectionOverrides`                     | 已交付 |
| `setPluginAgentModelOverride`                 | `pluginAgentModelSelectionOverrides`                 | 已交付 |
| `list`                                        | 内置 + 用户 + 工作区 + 插件 agent 的发现与运行优先级 | 待做   |
| `createAgent` / `updateAgent` / `deleteAgent` | agent markdown 的序列化、写盘与禁用态迁移            | 待做   |

**待做部分的前置**（本片已核实，都是 Host 本地能力、无上游缺口，但它们合起来构成明显更大的另一半）：

- `subagentMarkdown.ts`（435 行）：frontmatter ↔ `SubAgentConfig` 的解析与序列化 + 一份 legacy 迁移；
- 多来源发现：内置两个 agent、用户根、工作区根、**插件清单**（`.escode-plugin` / `.claude-plugin` /
  `.codex-plugin` 三种）、`installed_plugins.json`、官方缓存、`package.json` 里的 agent 目录声明；
- `subagentConfigScope.ts`（343 行）：chokidar 式的配置作用域——锚点回溯（根目录被删时监听最近的现存祖先）、
  父目录监听、写稳定等待（1s 稳定 + 500ms 轮询）、事件去抖 300ms、按路径增量重载、代际（revision）防旧结果覆盖；
- `subagentRuntimeConfig.ts`（230 行）：三个作用域（用户 / 状态 / 项目）的共享缓存与首次加载 5s 超时回退。

## 2. 路径与状态（本片建立）

| 项              | 约定                                                                                                                                  |
| --------------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| 存储根          | `config.storage.dir`（trim 后非空）否则 `~/.escode`；`~/` 展开到 home、绝对路径原样、其余按进程 cwd（`core_api::env::current_dir()`） |
| 用户 agent 根   | `<存储根>/agents`                                                                                                                     |
| 工作区 agent 根 | `<workspacePath>/.escode/agents`                                                                                                      |
| 状态文件        | `<存储根>/v2/agents-state.json`                                                                                                       |

**与 `commands` 的差别**：那里定位的是 `<存储根>/cli`，这里是**存储根本身**。

状态文件落盘 = `JSON.stringify(state, null, 2)`，**没有结尾换行**（`commands` 的 config.json 有）。

### 2.1 读入归一（`agents-state.json`）

- `builtInModelSelectionOverrides`：只认 `general-purpose` / `Explore` 两个名字，条目必须通过 modelSelection 校验；
- `pluginAgentModelSelectionOverrides`：只认 `plugin:` 前缀的 id，条目同样要校验；
- `disabledAgentIds`：只留**非空**字符串（trim 后判定），保持原顺序（去重与排序由 `setEnabled` 负责）；
- 其余字段（含未知字段与 legacy 的 `builtInModelOverrides` / `builtInThoughtLevelOverrides`）原样保留；
- 文件缺失、JSON 语法错误、非对象 → 空状态（不抛）；其它 IO 错误照常抛。

### 2.2 `modelSelection` 校验（`model_selection.rs`）

`providerId` / `modelId` 必须 `trim()` 后非空；`options` 是 **strict** 的 `{reasoningLevel?: string}`；
`reasoningLevel` 为空时不带 `options` 键；`null` 或缺省表示**清除覆盖**。

## 3. 写入面

- **所有写操作串行**（TS 的 `enqueue` 写队列）：读-改-写是一个整体，否则并发写会丢更新。
- `setEnabled`：禁用即加入、启用即移除；写回时 `disabledAgentIds` **去重并升序**。
- 两个模型覆盖：有值写入、无值删键；**不会清空整张表**，也不动其它条目。
- `setPluginAgentModelOverride` 要求 id 以 `plugin:` 开头**且 trim 后与自身相等**，否则
  `无效插件 Subagent 身份`（错误文案逐字，Renderer 直接展示）。
- `getPrimaryUserAgentsDirectory` 的 `provider` 入参**不参与路径**（TS 形参 `_params`），照常忽略。

## 4. 登记差异

1. **`agents-state.json` 的键序**：`serde_json` 未启用 `preserve_order`，按键名字典序输出；TS 保持插入序。
   两侧互读语义相同（键是映射），仅逐字节比较可见。
2. **不做 legacy 状态迁移**：TS 的 `readAgentStateAtPath` 会先跑 `migrateSubagentStateFile`，把
   `builtInModelOverrides` + `builtInThoughtLevelOverrides` 折叠进 `builtInModelSelectionOverrides` 并改写文件。
   本片不迁移：**旧字段原样保留、不被读取**，`builtInModelSelectionOverrides` 里只会出现新写法的条目。
   可见条件：存在 legacy 状态文件的用户——`list` 之前（本片尚未交付）其内置覆盖不生效；随 `list` 一起补。
3. **并发写入的跨进程互斥**：TS 的写队列是**进程内**的；`writeAgentStateFile` 用 `atomicWriteText`
   （无文件锁）。Rust 同样只有进程内串行 + 原子 rename——双 Host 交替写同一文件时与 TS 同风险，未加强。
4. JSON 之外的错误文案：`modelSelection` 校验失败在 TS 是 `ZodError` 文案，Rust 只回一句话
   （只比接受/拒绝，与 `plugin_result_schema` 同口径）。

## 5. 验收

- 单测 12 例：`storage_tests` 2（缺省根与派生路径、`storage.dir` 的 `~/`/绝对/空白/坏 JSON）、
  `model_selection_tests` 3（trim 与重建、非法拒绝、null 清除）、
  `state_tests` 3（缺失/坏 JSON/非对象→空、落盘无结尾换行、读入归一与未知字段保留）、
  `mod_tests` 4（目录落盘与幂等、禁用集去重升序、内置覆盖写入与清除、插件 id 校验与写入清除）。
- 端到端 1 例（真 WebSocket）：目录解析 → 禁用集（含去重升序）→ 内置覆盖写入（trim）与清除 →
  插件 id 校验错误 → 未迁移 4 个成员的显式缺口且列出已实现集合。
- 负向验证两项（均已恢复）：①`setEnabled` 去掉排序 → 逆序插入得到 `["b","a"]`，用例失败；
  ②读入不归一 → 非法内置条目被保留，用例失败。
  （第一版排序用例写成「b→a→b」三次写入，第三次恰好把顺序修正，负向验证发现它测不到排序，已改为「b→a」。）
