# Rust runtime：Tool Search 与 Deferred Tool Call（W2）

> 状态：已实现（local + native，2026-10-28；端到端覆盖 2026-10-31；未发现 schema 纠错 2026-11-01）
> 关联：[upstream-v3.16.2-sync.md](upstream-v3.16.2-sync.md)、[rust-tool-surface.md](rust-tool-surface.md)、[rust-tool-schema-order.md](rust-tool-schema-order.md)、[rust-tool-input-validation.md](rust-tool-input-validation.md)
> 上游参考：`apps/zcode-cli/packages/contracts/src/tools/tool-search.ts`、`.../deferred-tool-call.ts`、`packages/core/src/tool/search/*`、`packages/core/src/tool/handlers/{tool-search,deferred-tool-call}.ts`

## 1. 行为

上游 v3.16.2 新增两段模型可见控制面：

- **ToolSearch**：在“延迟工具目录”里按关键词或 `select:<tool_name>` 查找工具，返回
  `{ matches, query, total_deferred_tools }`；local 模式在同一工具结果里用 `<functions>` 块展开命中工具的
  完整 schema，native 模式改回 `tool_reference`；命中的工具从下一轮起以完整 schema 进入模型工具面。
- **DeferredToolCall**：按 `{name, input}` 调用一个延迟工具。是否已发现不是准入条件（上游
  `resolveDeferredToolCall` 只校验包装入参、目录归属与 `deferred` 标记）：未发现时仍解包成目标工具调用，
  按注册表里的真实 schema 校验入参，校验失败再回灌下文 §5.3 的纠错提示。模型若直接按名称调用，
  同样走普通工具调度路径（local 模式下该工具不在工具面，通常不会被模型选中）。

模式：

| 模式     | 行为                                                                                                           |
| -------- | -------------------------------------------------------------------------------------------------------------- |
| `off`    | 完全关闭，工具面与现状一致                                                                                     |
| `local`  | 客户端实现 ToolSearch / DeferredToolCall，延迟工具不直接出现在模型工具面                                       |
| `native` | 使用 provider 原生 tool search（Anthropic `advanced-tool-use-2025-11-20`），客户端只返回 `tool_reference`      |
| `auto`   | 模型声明 `requestCapabilities.toolSearch.defaultEnabled` 时解析为 `native`，否则 `local`（无能力时等价 `off`） |

硬约束：

- `native` 仅允许声明 `protocolSupported` 的 Anthropic Messages provider；其他 provider 显式报错，不静默降级。
- `off` 不改变现有工具面、请求形态或权限。
- 删除域不恢复：channel reply、off-peak、Web 远控、分享、Bots 不进入目录。
- 延迟工具仍按自己的权限规则审批；DeferredToolCall 的高风险审批不能替代目标工具的审批。

## 2. 状态所有者

| 状态                                                          | owner                                    |
| ------------------------------------------------------------- | ---------------------------------------- |
| `toolSearchMode`（off/local/native/auto）                     | 会话 runtime preferences / CLI 参数      |
| 模型能力 `requestCapabilities.toolSearch`                     | 模型目录                                 |
| 延迟工具目录（name / aliases / mcp / description / deferred） | runtime 工具注册表                       |
| 已发现工具集合（discovered）                                  | runtime session 状态，按会话持久化       |
| `addedNames` / `removedNames` / `readdedNames`                | runtime 每轮根据 discovered 与可见性计算 |

事件顺序（local）：

```
prepare_tool_surface
  ├─ 注册表按现有规则过滤（工作流/cron/WebSearch/技能门）
  ├─ 若 mode=local：resident 直接发给模型
  │   deferred 不发给模型，加入 catalog
  │   注入 ToolSearch + DeferredToolCall
  └─ 上一轮 discovered 命中的工具以完整 schema 发给模型

tool call = ToolSearch(query)
  └─ 返回 { matches, query, total_deferred_tools }
     local 结果同时带命中工具的 <functions> schema 块；native 带 tool_reference 块
     matches 同时并集进 discovered，下一轮展开 schema

tool call = DeferredToolCall({name,input})
  ├─ name 必须属于 deferred catalog（否则交回模型：只能调用延迟工具）
  └─ 重写为 name 的正常调用，走原权限/校验/执行路径
     未发现时校验失败 → 追加 §5.3 的纠错提示（退化原因 + 当前 schema + 精确加载方式）
```

## 3. 接口

- `ToolSearch` 入参：`query`（必填）、`max_results`（默认 5，模型侧 required）。
- `ToolSearch` 出参：`{ matches: string[], query: string, total_deferred_tools: number }`。
  local 模式另附 `<functions>` 文本块，每个命中一条
  `<function>{"description","name","parameters"}</function>`（定义缺失的名字跳过，全部缺失时不输出空容器）；
  native 模式另附 `tool_reference` 块。
- `DeferredToolCall` 入参：`{ name: string, input: object }`，无独立出参（透传目标工具结果）。
- 模型内容：local 模式返回文本结果 + `<functions>` schema 块；native 模式返回 `tool_reference` 内容块。
- 控制工具描述与 schema（2026-11-01 对齐上游 `packages/core/src/tool/search/description.ts`、
  `packages/contracts/src/tools/{tool-search,deferred-tool-call}.ts`）：
  - 两种模式共用 `description.ts` 模板（`<system-reminder>`、`Result format` 段、Query forms）；
    `native` 用 “Once a tool's schema appears …” 与 “— the same encoding as the tool list at the top of this prompt.”，
    `local` 用 “Call deferred tools through DeferredToolCall … even if their definitions were returned by
    direct selection.”、“. For deferred tools, parameters describes the input object passed to DeferredToolCall.”
    并追加 `Example (TodoWrite is deferred and already loaded)` 块。
  - `ToolSearch` schema：`required = [query, max_results]`，`max_results` 带 `default: 5`；非 strict 对象，
    不带 `additionalProperties: false`（上游 zodToJsonSchema 产物）。
  - `DeferredToolCall` schema：strict 对象，`additionalProperties: false`；`input` 为
    `{"type":"object","additionalProperties":{}}`（上游 `z.record(z.unknown())`）。
  - 以上文案与字段只影响模型可见请求；不改变执行语义与权限。
- 公告（2026-11-01 已实现，见 §5.1）：每轮向模型通告 `addedNames` / `removedNames` /
  `readdedNames`；`readdedNames ⊆ addedNames`。

## 4. Rust 集成点

| 位置                                                             | 改动                                                                       |
| ---------------------------------------------------------------- | -------------------------------------------------------------------------- |
| `apps/escode-cli-rust/crates/tools/src/tool_surface.rs`          | 注入两个控制工具定义；按模式拆分 resident/deferred                         |
| `apps/escode-cli-rust/crates/domain/src/tool_search_definitions.rs` | `ToolSearch` / `DeferredToolCall` 描述与 schema（§3）                    |
| `apps/escode-cli-rust/crates/core/src/app/agent_loop_surface.rs` | 计算本轮 catalog 与 discovered 展开                                        |
| `apps/escode-cli-rust/crates/core/src/app/tool_dispatch.rs`      | 拦截 ToolSearch（local 结果带 schema 块）；DeferredToolCall 校验后重写调用 |
| `apps/escode-cli-rust/crates/core/src/app/context.rs`            | 会话级 discovered 集合与 mode                                              |
| `apps/escode-cli-rust/crates/model`                              | Anthropic 请求：native 模式 `tool_search` 与 `tool_reference` 内容块       |
| `apps/escode-cli-rust/crates/protocol`                           | 不新增帧；模式来自已有 runtime preferences / 命令参数                      |

搜索算法（local）：

1. `select:<name>[,<name>...]`：精确命中，按名称排序，支持别名与 MCP `serverName/toolName`。
2. 关键词：对名称、别名、描述、`searchHint` 做大小写不敏感匹配；名称命中优先于描述命中。
3. 结果去重、最多 `max_results` 条，空结果返回 `No matching deferred tools found`。
4. 搜索是纯函数，不读文件、不发网络请求，10s 预算内。

## 5. Rust 实现状态（2026-10-28；schema 结果投影与端到端覆盖 2026-10-31）

已实现（local）：

- 纯函数目录/搜索/DeferredToolCall 解析：`crates/domain/src/tool_search_catalog.rs`。
- 控制工具定义与会话状态：`crates/core/src/app/agent_loop_surface.rs`、`crates/core/src/app/tool_dispatch.rs`、`crates/tools/src/workspace_tools.rs`。
- 结果投影：local ToolSearch 只为“批次开始前尚未交付”的延迟工具（以及常驻工具）展开完整
  `function` 定义；已可见的延迟工具不再重复展开，只给“already available”提醒。定义缺失的引用
  直接丢弃（上游 `projectToolReferences`），全部缺失时不输出空容器；匹配 schema 不进入顶层 tools。
- 结果提醒（2026-11-01，对齐上游 `apps/zcode-cli/packages/core/src/tool/search/local-projection.ts`）：
  schema 块后依次追加 `<system-reminder>` 包装的“本次新加载”提醒
  （`Loaded: … Call it/them now with DeferredToolCall …`）和“会话内已加载”提醒
  （`Tool definitions already available in this conversation: … Searching again does not execute them.`）。
  是否已加载按本轮批次开始时的已发现快照判定，同一批并行搜索互不算已加载；已可见的延迟工具
  不再重复展开 schema，常驻工具始终展开且不产生两种提醒。
- 已发现事实持久化：成功 ToolSearch 结果消息顶层写 `_escode_tool_search_discovered=[name…]`
  （失败或无命中不写），local 冷恢复与压缩边界都从历史重建已发现集合。
- 权限元数据：`crates/tools/src/tool_capability.rs`。
- 入口：`toolSearchMode` 命令字段（V4 输入）或 `ESCODE_TOOL_SEARCH_MODE` 环境变量；未设置时为 `off`。App 偏好接入后改为读设置。
- 测试：`cargo test -p escode-cli-domain tool_search`、`--test tool_search_differential`、`cargo test -p escode-cli-model tool_search`、`cargo test -p escode-cli-tools tool_search_state`。

已实现（native）：

- 请求侧：`advanced-tool-use-2025-11-20` beta、`deferLoading` → `defer_loading`（`crates/model/src/tool_search.rs`、`anthropic_body.rs`、`provider.rs`）。
- 结果侧：ToolSearch 命中在 native 模式返回 `tool_reference` part；工具结果以 part 数组入史，Anthropic body 投影为 `tool_result` 内的 `{"type":"tool_reference","tool_name":...}`（`model_media.rs`、`tool_reference.rs`）。
- 成功提示（2026-11-01，对齐上游 `appendToolSearchSuccessMessages`）：native 请求投影在每段连续
  tool 结果之后，按该段里成功 ToolSearch（结果仍含 `tool_reference`）的条数追加
  `role=user`、`content="Tool loaded."` 的派生消息。提示只存在于请求投影，不写入历史；
  `local`/`off` 的结果没有 `tool_reference`，不产生该提示。
- 跨模式：会话从 native 切到 local/off 或另一家 provider 时，历史里的 `tool_reference` 在发送前降级为文本，避免未知 part 被拒；native 请求保留仍可用的引用，删除本轮目录里已不存在的工具引用并按上游写入占位文本（`tool_reference::project`）。
- 包装边界：DeferredToolCall 的目标结果在模型历史/持久化工具消息上带 `_escode_model_call_wrapper: "DeferredToolCall"`，供 UI 与冷恢复区分直接调用与包装调用。
- `auto` 是否解析为 native 取决于 provider 的 `default_enabled`；当前 Rust 默认 `false`，显式 `native` 可用。

差分：

- `crates/domain/tests/fixtures/tool_search_cases.json` 由上游 `search.ts` 生成（12 个 query），Rust 集成测试逐条比对。
- `crates/domain/tests/fixtures/tool_search_cases_realistic.json` 用 Rust `tool_surface.json` 的真实工具名/描述 + 6 个 MCP 工具生成（17 个 query），覆盖 cron/workflow/MCP 名称的 tokenization。
- 实时差分：`packages/services/tests/escode-cli-rust-tool-search.test.ts` 跑上游 `search.ts` oracle 与 Rust `examples/tool_search_fixture`，对同一 catalog/query 比对（19 cases）。
- 端到端覆盖（2026-10-31）：`packages/services/tests/escode-cli-rust-tool-search-e2e.test.ts` 用真实 stdio MCP server
  - 模型 fixture 验证 local 首轮隐藏 MCP 工具、ToolSearch 命中结果含 `<functions>` schema 且当轮工具面不变、
    DeferredToolCall 执行、未发现时拒绝且不执行目标、下一用户轮展开 schema，以及 off 基线不注入控制工具。
- 本 fork 的 Node CLI 把 `toolSearchMode` 固定为 `off`
  （`apps/escode-cli/packages/bootstrap/src/escode-protocol/server-operations.ts:3104`），没有 Node 端到端可差分；
  搜索算法继续与上游 `search.ts` oracle 逐条比对。
- 目录增删/恢复公告（2026-11-01 已实现，设计见 §5.1）：`crates/domain/src/tool_search_announcement.rs`
  按上游文案与增量规则生成公告消息，core 在步骤结束时落盘并投影到下一轮输入。
- 公告 oracle 现状（2026-11-01 复核）：上游 `upstream/main`（`zai-org/ZCode`）已有完整实现与用例——
  `apps/zcode-cli/packages/core/src/tool/search/exposure.ts` 的 `buildToolSearchAnnouncement`（增量计算、
  `LOCAL_ADDED_TOOLS_NOTICE` / `ADDED_TOOLS_NOTICE`、30 行删除阈值、段落间空行与 MCP 名称聚合）、
  `packages/core/src/tool/search/history.ts`（`collectToolSearchHistory` / `projectToolReferences`）、
  `packages/contracts/src/tools/tool-search.ts` 的 `ToolSearchAnnouncementSchema`，以及
  `packages/core/tests/tool-search-additions.test.ts`、`tool-search-announcement-persistence.test.ts`、
  `tool-search-announcement-runtime.test.ts`。
- 本 fork 的 TS CLI 树没有 `apps/escode-cli/packages/core/src/tool/search/`（`toolSearchMode` 固定 `off`），
  仓库内仍没有可直接运行的 Node oracle。Rust 实现逐字沿用上述上游模块的文案、阈值与排序规则，
  并由 domain 单测（增量规则、30/31 删除阈值、MCP 分组）与 `escode-cli-rust-tool-search-e2e.test.ts` 覆盖；
  在把上游模块同步进 fork 之前，不宣称与本 fork Node runtime 做过帧级差分。

### 5.1 目录公告实现设计（2026-11-01）

文案、增量规则与 metadata 键逐字对齐上游 `buildToolSearchAnnouncement`，不新增配置或实验开关。

状态与所有者：

- 公告事实的唯一 owner 是会话消息：`role=user`、正文 `<system-reminder>…</system-reminder>`、
  `_escode_source="tool_search_announcement"`、
  `_escode_tool_search_announcement={addedNames,removedNames,readdedNames}`。
  `readdedNames ⊆ addedNames`；名字事实不进入会话元数据表或运行时缓存。
- 已发现事实的唯一 owner 同样是会话消息：`native` 用 ToolSearch 结果的 `tool_reference` part，
  `local` 用同一结果消息顶层的 `_escode_tool_search_discovered`（成功命中才写）。冷恢复从这两处
  与 compact 边界重建 `discovered`；`ToolPort` 的进程内记录只是缓存。
- `announced` / `listed` 每次从保留消息重新扫描：`addedNames` 加入 `announced`，非 `readdedNames` 的
  加入 `listed`；`removedNames` 从 `announced` 删除。
- `deferred` 来自本轮 ToolSearch catalog（`prepare_tool_surface` 过滤前的目录），
  `available` 来自本轮注册目录（过滤前的 `registered` 快照）。两者都由 core 注入，domain 不读全局状态。

增量规则（与上游一致）：

- `addedNames = deferred − announced ∪ (deferred ∩ announced ∩ ¬listed)`，即“新增”或“曾被压缩掉列名”；
  `readdedNames = deferred ∩ ¬announced ∩ listed`（此前公告过、随后移除、现在恢复）。
- `removedNames = announced − available`，按完整当前作用域判定；deferred→alwaysLoad 仍算可用，不公告移除。
- 无 added、无 removed 时不写公告（纯 Continue / 无变化轮）。

渲染：

- 新增段逐行列出名字：`native` 用 `The following deferred tools are now available via ToolSearch…`，
  `local` 用 `The following deferred tools are available via ToolSearch…`；`local` 文案同时说明
  `DeferredToolCall` 与常驻工具可直接调用。
- 恢复段按 MCP server 聚合（`mcp__server__*`，带计数），只保留一句
  `N deferred tool(s) … available again … Load via ToolSearch as before.`。
- 删除段在 ≤30 个名字时逐行列出，>30 时按同一分组聚合；删除段后追加 ambient-context 提示。
  各段之间用空行分隔。
- 聚合排序按 `localeCompare` 语义（大小写不敏感、再按原串兜底），名字列表按 UTF-16 默认排序。

投影与恢复：

- 关闭 ToolSearch 时只在请求投影中隐藏公告消息，不删除存储事实（`RunContext.projection`）。
- 公告在请求投影里按上游 bubble 规则移到本轮真实输入之前：跨过同轮其它 reminder，
  遇到工具/助手消息即停（公告仍留在它所属的步骤之后）。
- 已发现集合在会话冷恢复时从历史 `tool_reference` part、`_escode_tool_search_discovered` 与
  compact 边界重新派生，并回填 `ToolPort`，与进程内记录取并集。公告只记录目录变化，
  不承载 schema 交付事实，不能替代已发现集合。

压缩边界（2026-11-01 实现，见 §5.2）：

- 已发现工具名随压缩边界持久化，冷恢复后并入 `discovered`，与上游
  `compactBoundary.preCompactDiscoveredTools` 语义一致。

### 5.2 压缩边界中的已发现工具（2026-11-01）

所有者与持久化：

- 已发现工具名存进 `ContextState.preCompactDiscoveredTools`（`crates/domain/src/context.rs`），
  随会话 `context` 持久化（`rust_session.body`）。不新增表列、migration 或缓存，旧行缺字段按空集合读入，
  空集合不写出字段。
- 唯一写入者是 `RunContext::compact()` 成功路径：`ContextPort` 事务提交后才切换边界与工作副本；
  失败时边界与已发现集合都不变。
- 取值 = 当前 canonical 历史里的 `tool_reference` / `_escode_tool_search_discovered`、上一检查点的
  边界名单、`ToolPort` 本进程已发现集合的并集，排序去重后写入。关闭 ToolSearch 时同样写入（上游 compact 语义）。

读取：

- `prepare_tool_surface` 冷恢复时把边界名单并入 `discovered` 并回填 `ToolPort`，与历史
  `tool_reference` / `_escode_tool_search_discovered` 取并集。
- `local` 模式把边界名单里的工具视为已发现：本轮工具面保留其定义，catalog 的 `deferred` 标记清除，
  公告生成因此不会把它们重新算作新增目录项；`native` 模式维持原有 `deferLoading` 规则。
- `ReadSessionContext` 的合成 compaction part 不写完整 `CompactBoundaryPayload`：Rust 侧没有
  `boundaryId`/`traceId` 等价事实，且当前 App 与 TS CLI 都不消费该字段（`toolSearchMode` 固定 `off`）。
  需要 App 展示边界时再补完整 payload。

不做：不把摘要正文或公告正文写进边界；不从搜索结果推测已发现；不改变 compact 摘要请求本身。

验收：

- 旧 `ContextState` JSON（无字段）读入为空集合；写入后序列化含排序去重的名单。
- E2E：local ToolSearch 下经 ToolSearch 加载 MCP 工具 → 手动 `/compact` → 关闭 runtime 冷恢复 →
  下一轮工具面仍包含该工具定义，公告不把它当作新增项。（`escode-cli-rust-tool-search-e2e.test.ts`）
- E2E：local ToolSearch 加载 MCP 工具后不压缩直接冷恢复（`session/resume` 重新带上 MCP 配置）→
  下一轮工具面仍包含该工具定义，公告不重复。（同一测试文件）

### 5.3 未发现延迟工具的入参校验纠错（2026-11-01）

上游 `apps/zcode-cli/packages/core/src/tool/executor/call-runner.ts` 的
`UNDISCOVERED_TOOL_SCHEMA_EXPLANATION`：延迟工具尚未进入已发现集合时，模型提示里没有它的 schema，
类型化参数（数组、数字、布尔）会退化成字符串，然后被客户端解析器拒绝。只有校验失败时才追加纠错，
成功调用不追加。

触发条件（与上游一致，三者同时成立）：

- ToolSearch 已启用（`tool_search` 非空，即 `local` / `native`）；
- 目标工具在 catalog 里且 `deferred == true`；
- 目标工具不在 `discovered`（用运行时实时集合，不用批次开始的 `visible` 快照）。

文案（逐字对齐上游）：

```
\n\nThis tool's schema was not sent to the API — it was not in the discovered-tool set derived from message history. Without the schema in your prompt, typed parameters (arrays, numbers, booleans) get emitted as strings and the client-side parser rejects them. Load the tool first: call ToolSearch with query "select:<name>", then retry this call. For reference, this tool's input schema is: <schema>
```

- `<schema>` 是注册表定义里的 `function.parameters`，按声明顺序紧凑输出（`schema_order::ordered`
  有登记时用声明顺序，否则回落排序形式），等价上游 `JSON.stringify(entry.inputSchema)`。
- `local` 模式末尾再加一行：`Call DeferredToolCall with {name: "<name>", input: {...}}.`；`native` 不加。
- 注册表里没有该工具（`Tool not found` 已在更早的注册表查找处返回）时不追加，不伪造 schema。
- 提示追加在校验失败文案之后，不改变失败判定、不请求权限、不执行目标工具。

入参校验的 schema 来源（2026-11-01 收口）：统一用本会话注册表快照 `registered`（上游
`deps.registry.get(name).inputSchema`），不再用本轮模型可见面 `definitions`。两者对可见工具逐字相同；
差别只在被可见性隐藏但仍在注册表里的工具（local 未发现的延迟工具、Cron/WebSearch 隐藏项）——
这些工具现在也按真实 schema 校验，此前会跳过校验直接执行。

验收：

- domain 单测：`resolve_deferred_call` 不因未发现而拒绝；guidance 文案（native/local 后缀、schema 文本）逐字匹配。
- E2E（local）：未发现时直接调用 MCP 工具或 `DeferredToolCall` 且缺必填参数 → 结果含退化原因、
  `select:<name>`、当前 schema 与 `DeferredToolCall` 提示；成功入参不追加提示。

## 6. 验收

- `off`：工具面、请求体、权限与当前完全一致（golden 对比）。
- `local`：延迟工具不出现在首轮工具面；ToolSearch 结果内带命中工具的完整 schema，顶层 tools 不新增目标定义；
  下一轮出现；DeferredToolCall 可执行且走目标工具权限。
- 未发现的 DeferredToolCall：解包为目标工具调用；入参合法则执行，非法则返回 §5.3 纠错提示且不执行目标工具。
- 未发现直接调用（`native` 与 `local`）：校验失败追加 §5.3 提示；已发现或关闭 ToolSearch 时文案与普通校验失败一致。
- `native`：非 Anthropic provider 显式报错；Anthropic 请求包含 `advanced-tool-use-2025-11-20` beta 与 `tool_reference`。
- 成功提示：`cargo test -p escode-cli-model tool_search` 覆盖连续 tool 结果段后的 `Tool loaded.`
  追加位置与条数；失败结果、local 文本结果与无引用结果不追加。
- `auto`：能力缺失时解析为 `off`，有 `defaultEnabled` 时解析为 `native`。
- 删除域不出现在 catalog。
- 差分：同一 fixture 下 Node oracle 与 Rust 的 `matches`、`total_deferred_tools`、公告 delta 一致。
- 公告（2026-11-01，`escode-cli-rust-tool-search-e2e.test.ts`）：local 首轮注入一次新增公告并逐行列出
  MCP 工具，下一用户轮不重复；`off` 不产生公告。domain 用例覆盖 30/31 删除阈值、MCP 分组恢复段、
  压缩后重建与“恢复提示不算完整列名”的后缀补齐。
- 结果提醒（2026-11-01，同一 E2E）：首次命中追加 `Loaded: …` 调用提醒；再次搜索同一延迟工具时追加
  “already available in this conversation … Searching again does not execute them” 提醒。
- `pnpm typecheck`、`pnpm lint`、`pnpm check:escode-cli-rust`、`pnpm test:escode-cli-rust` 通过。

## 7. 非目标

- 不实现 provider-specific tool search 之外的搜索后端。
- 不恢复 channel reply / off-peak 等删除工具。
- 不改现有权限模型；DeferredToolCall 不绕过目标工具审批。
