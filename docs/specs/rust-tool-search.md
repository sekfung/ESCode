# Rust runtime：Tool Search 与 Deferred Tool Call（W2）

> 状态：待实现 · 2026-10-28
> 关联：[upstream-v3.16.2-sync.md](upstream-v3.16.2-sync.md)、[rust-tool-surface.md](rust-tool-surface.md)、[rust-tool-schema-order.md](rust-tool-schema-order.md)、[rust-tool-input-validation.md](rust-tool-input-validation.md)
> 上游参考：`apps/zcode-cli/packages/contracts/src/tools/tool-search.ts`、`.../deferred-tool-call.ts`、`packages/core/src/tool/search/*`、`packages/core/src/tool/handlers/{tool-search,deferred-tool-call}.ts`

## 1. 行为

上游 v3.16.2 新增两段模型可见控制面：

- **ToolSearch**：在“延迟工具目录”里按关键词或 `select:<tool_name>` 查找工具，返回匹配的工具名；命中的工具在后续轮次按需加载完整 schema。
- **DeferredToolCall**：在 schema 已加载后，按 `{name, input}` 立即调用一个延迟工具；未通过 ToolSearch 加载的工具不能直接调用。

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
     matches 同时并集进 discovered，下一轮展开 schema

tool call = DeferredToolCall({name,input})
  ├─ name 必须在 discovered 且属于 deferred catalog
  ├─ 否 → 交回模型：先调用 ToolSearch 加载
  └─ 是 → 重写为 name 的正常调用，走原权限/校验/执行路径
```

## 3. 接口

- `ToolSearch` 入参：`query`（必填）、`max_results`（默认 5，模型侧 required）。
- `ToolSearch` 出参：`{ matches: string[], query: string, total_deferred_tools: number }`。
- `DeferredToolCall` 入参：`{ name: string, input: object }`，无独立出参（透传目标工具结果）。
- 模型内容：local 模式返回文本结果；native 模式返回 `tool_reference` 内容块。
- 公告：每轮向模型通告 `addedNames` / `removedNames` / `readdedNames`；`readdedNames ⊆ addedNames`。

## 4. Rust 集成点

| 位置                                                             | 改动                                                                 |
| ---------------------------------------------------------------- | -------------------------------------------------------------------- |
| `apps/escode-cli-rust/crates/tools/src/tool_surface.rs`          | 注入两个控制工具定义；按模式拆分 resident/deferred                   |
| `apps/escode-cli-rust/crates/tools/src/tool_schemas.json`        | 从 TS 契约生成 `ToolSearch` / `DeferredToolCall` schema              |
| `apps/escode-cli-rust/crates/core/src/app/agent_loop_surface.rs` | 计算本轮 catalog、discovered 展开、公告 delta                        |
| `apps/escode-cli-rust/crates/core/src/app/tool_dispatch.rs`      | 拦截 ToolSearch；DeferredToolCall 校验 discovered 后重写调用         |
| `apps/escode-cli-rust/crates/core/src/app/context.rs`            | 会话级 discovered 集合与 mode                                        |
| `apps/escode-cli-rust/crates/model`                              | Anthropic 请求：native 模式 `tool_search` 与 `tool_reference` 内容块 |
| `apps/escode-cli-rust/crates/protocol`                           | 不新增帧；模式来自已有 runtime preferences / 命令参数                |

搜索算法（local）：

1. `select:<name>[,<name>...]`：精确命中，按名称排序，支持别名与 MCP `serverName/toolName`。
2. 关键词：对名称、别名、描述、`searchHint` 做大小写不敏感匹配；名称命中优先于描述命中。
3. 结果去重、最多 `max_results` 条，空结果返回 `No matching deferred tools found`。
4. 搜索是纯函数，不读文件、不发网络请求，10s 预算内。

## 5. Rust 实现状态（2026-10-28）

已实现（local）：

- 纯函数目录/搜索/DeferredToolCall 解析：`crates/domain/src/tool_search_catalog.rs`。
- 控制工具定义与会话状态：`crates/core/src/app/agent_loop_surface.rs`、`crates/core/src/app/tool_dispatch.rs`、`crates/tools/src/workspace_tools.rs`。
- 权限元数据：`crates/tools/src/tool_capability.rs`。
- 入口：`toolSearchMode` 命令字段（V4 输入）或 `ESCODE_TOOL_SEARCH_MODE` 环境变量；未设置时为 `off`。App 偏好接入后改为读设置。
- 测试：`cargo test -p escode-cli-domain tool_search`、`--test tool_search_differential`、`cargo test -p escode-cli-model tool_search`、`cargo test -p escode-cli-tools tool_search_state`。

已实现（native）：

- 请求侧：`advanced-tool-use-2025-11-20` beta、`deferLoading` → `defer_loading`（`crates/model/src/tool_search.rs`、`anthropic_body.rs`、`provider.rs`）。
- 结果侧：ToolSearch 命中在 native 模式返回 `tool_reference` part；工具结果以 part 数组入史，Anthropic body 投影为 `tool_result` 内的 `{"type":"tool_reference","tool_name":...}`（`model_media.rs`、`tool_reference.rs`）。
- 跨模式：会话从 native 切到 local/off 或另一家 provider 时，历史里的 `tool_reference` 在发送前降级为文本，避免未知 part 被拒（`tool_reference::project`）。
- 包装边界：DeferredToolCall 的目标结果在模型历史/持久化工具消息上带 `_escode_model_call_wrapper: "DeferredToolCall"`，供 UI 与冷恢复区分直接调用与包装调用。
- `auto` 是否解析为 native 取决于 provider 的 `default_enabled`；当前 Rust 默认 `false`，显式 `native` 可用。

差分：

- `crates/domain/tests/fixtures/tool_search_cases.json` 由上游 `search.ts` 生成（12 个 query），Rust 集成测试逐条比对。
- `crates/domain/tests/fixtures/tool_search_cases_realistic.json` 用 Rust `tool_surface.json` 的真实工具名/描述 + 6 个 MCP 工具生成（17 个 query），覆盖 cron/workflow/MCP 名称的 tokenization。
- 实时差分：`packages/services/tests/escode-cli-rust-tool-search.test.ts` 跑上游 `search.ts` oracle 与 Rust `examples/tool_search_fixture`，对同一 catalog/query 比对（19 cases）。
- Node 端到端差分（真实 MCP server + 模型 fixture）尚未接入；这是测试覆盖缺口，不是功能缺口。

## 6. 验收

- `off`：工具面、请求体、权限与当前完全一致（golden 对比）。
- `local`：延迟工具不出现在首轮工具面；ToolSearch 命中后下一轮出现；DeferredToolCall 可执行且走目标工具权限。
- 未发现的 DeferredToolCall：返回可执行提示，不执行目标工具。
- `native`：非 Anthropic provider 显式报错；Anthropic 请求包含 `advanced-tool-use-2025-11-20` beta 与 `tool_reference`。
- `auto`：能力缺失时解析为 `off`，有 `defaultEnabled` 时解析为 `native`。
- 删除域不出现在 catalog。
- 差分：同一 fixture 下 Node oracle 与 Rust 的 `matches`、`total_deferred_tools`、公告 delta 一致。
- `pnpm typecheck`、`pnpm lint`、`pnpm check:escode-cli-rust`、`pnpm test:escode-cli-rust` 通过。

## 7. 非目标

- 不实现 provider-specific tool search 之外的搜索后端。
- 不恢复 channel reply / off-peak 等删除工具。
- 不改现有权限模型；DeferredToolCall 不绕过目标工具审批。
