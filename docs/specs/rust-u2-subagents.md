# Rust 阶段 2：`subagents` 域（rust-unified-host.md 子 spec）

> 状态：切片交付中 · 2026-10-17。**4/8 已交付**（目录解析 + 状态写入面），另有 `list` 的前置件——
> markdown 解析器（§6）已落地。
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

## 6. markdown 解析器（`list` 的前置件，2026-10-17）

TS 侧是**手写的 YAML 子集**（无第三方库）：`subagent-profile-frontmatter.ts` 243 行做切分与宽松解析，
`subagent-profile-parser.ts` 226 行做字段归一，`subagent-markdown-selection.ts` 的解析侧再做 `model` 解释。
Rust 对应 `subagents/{frontmatter,profile}.rs`。

### 6.1 frontmatter（`frontmatter.rs`）

- **切分**：去 BOM；首行 trim 后必须是 `---`；取紧随其后的第二个 `---`（`\r?\n` 都接受）；
  缺任一端整段都算 body。
- **宽松解析**：空行与 `#` 注释跳过；`- ` 开列表项；`key:` 空值先记空数组并开启列表（后续缩进续行视为
  **块状 mapping**，不当列表项）；`key: value` 支持内联 `[...]`/`{...}`、行内注释（`#` **前须有空白**才生效）、
  成对引号才去引号。
- **数值**：`^\d+$` 与数字字面量按 **i64**（JS 序列化回 `42` 而不是 `42.0`）；带小数点/指数才用 f64。
- **`mcpServers` 的四态**：未配置（外层 `None`）／裸值（`Some(None)`）／块状 mapping 或未加引号的
  inline mapping 项（`Some(None)`）／合法名字列表（`Some(Some(names))`）。只有最后一种能扩大 child MCP scope。
- **`tools` 裸值**：`tools:` 后没有列表项时整键丢弃（TS `delete values.tools`）。

### 6.2 profile（`profile.rs`）

- 缺 frontmatter / 缺 `name` / 缺 `description` → **失败**，文案逐字（含 `<path>` 或 `<inline>`）；
- `mcpServers` 非法 → 失败（`agent_invalid_mcp_servers`）；
- `memory` 非法 → **带 profile 的警告**（`agent_invalid_memory_scope`）；
- **项目级来源不透传 `permissionMode`**（仓库输入不能把 child runtime 提升到 bypass）；
- `description` 里的字面 `\n` 还原成换行；`systemPrompt` = body.trim()；
- `color` / `permissionMode` / `memory` 走白名单；`maxTurns` 只认正整数；
- `tools` / `disallowedTools` 逐个去掉 `(...)` 后缀后去空；`tools` 写成数组但归一为空时显式回 `[]`；
- `skills` 按逗号/空白切；`background` / `injectAgentsMd` 接受布尔与 `"true"`/`"false"`。

### 6.3 `model` 解释（解析侧）

- `inherit` / `main` / `sonnet` / `opus` / `haiku` → **不产生覆盖**；
- `custom:<provider>[:<model>]`（含 `builtin:a:b:c` 三段式与百分号转义）→ 直接解码；
- 否则按 `provider/model[$reasoning]` 解析，`$` 在首位或末位时**不当作档位**（留在 modelId 里）；
- `thoughtLevel` 非空时写入 `options.reasoningLevel`；
- 解析不出合法 `providerId`/`modelId` → 不产生覆盖（TS 的 zod 抛错被 `try/catch` 吞掉）。

**未做**（属写面）：`serializeSubagentMarkdown`、`migrateSubagentMarkdownProvider`（legacy provider id 迁移）。

### 6.4 验收

- 单测 9 例：切分（BOM/CRLF/缺分隔符）、标量与列表与行内注释（含引号内的 `#`）、
  `mcpServers` 四态、`tools` 裸值丢弃、完整 frontmatter 的全字段映射、缺字段的三条文案、
  项目级丢弃 `permissionMode` 与 `memory` 警告、`model` 的五种形态、`tools` 归一为空的 `[]`。
- 实现期修正：第一版把整数解析成 f64（`42.0`），导致 `maxTurns` 的 `as_u64()` 也失配——改走 i64 后两处同时修复。

### 6.5 `list` 剩余部分（下一步，已逐项读码）

> **2026-10-17 复核**：逐项读码后，剩余工作量比初估更大，且有两处必须先定的口径。

1. **Settings 侧用的是真正的 `yaml` 包**（`subagentMarkdown.ts` 的
   `parseRuntimeCompatibleFrontmatter`）：`{...yamlValues, ...looseValues, mcpServers: yaml 数组优先}`。
   而 runtime 侧（`subagent-profile-parser.ts`）只用宽松解析。两条路径本来就不同。
   **待定**：Rust 的 `list` 是「只用宽松解析」（与 Rust runtime 自洽，登记差异）还是「用 saphyr 复刻 YAML 合并」
   （需一个 `saphyr → serde_json` 转换器，约 60 行，且要复刻 `yaml` 包对重复键报错等行为）。
2. **`createBuiltInAgents`**：两个内置 agent 的摘要是**常量**（`general-purpose` 蓝色 / `Explore` 青色，
   各自的 description / tools / injectAgentsMd / `readOnly: true` / `scope: builtin`），只需把
   `builtInModelSelectionOverrides` 回填到 `modelSelection` 与 `modelSelectionOverride`。
3. **文件发现**：用户根与工作区根、**只扫第一层**（插件才递归）、只收 `.*md`/`*.markdown`（大小写不敏感）、
   结果按 `localeCompare` 排序；读失败产出 `agent_read_failed` 诊断。
4. **插件发现**（最大的一块，约 230 行）：
   - 另一个缓存扫描器 `officialPluginCache.ts`（69 行）——返回**按插件名分组**的 `{name, versionRoots}`，
     版本目录按「数字感知、忽略大小写、降序」排序并过滤瞬态目录名；与 `commands` 用的那个**不是同一个**；
   - `installed_plugins.json` → 记录过滤（`enabledPlugins[id] === true`，且
     **卸载抑制优先于遗留安装记录**）；
   - 内置插件没有安装记录，故用缓存兜底（`enabledPlugins[id] ?? DEFAULT_ENABLED`），但缓存**不能复活**
     `suppressedBuiltins` 里的插件；
   - agent 根：`agents` + 清单 `agents` 字段（字符串或数组，去 `./` 前缀、必须仍在插件根内）；
   - 命名：`name = <pluginName>:<bareName>`，`id = plugin:<pluginId>:<bareName小写>`；
     裸名唯一且不与内置/已发现名字冲突时额外产出一条运行时**别名**，否则报 `agent_ambiguous_name`；
   - `applyPluginAgentOverrides`：插件文件只给默认值，用户覆盖替换**整份**选择（不混入原档位）。
5. **优先级与状态**：`applyRuntimePrecedence` 按 `name` 后者覆盖前者（Map 语义）；
   `attachEnabledState` 只对 `scope === "user"` 应用禁用集；`resolveCapabilities` 在桌面运行时恒为
   `{userScopeAvailable: true}`（Host 不读 env，直接按桌面口径）。
6. **legacy markdown 迁移**：**不做**（用户决定，2026-10-17）。解析侧已能识别 legacy 的 `builtin:a:b:c`
   与 `$` 档位，跳过迁移只会让文件里的 `model:` 行保持旧写法。**登记差异**：`list` 不再改写用户文件，
   属读路径无副作用（TS 每次 `list` 都会在文件锁下重写 legacy 文件）。

### 6.6 `list` 剩余部分（旧清单，保留供对照）
