# Rust 模型可见工具面与 Node 对齐

2026-09-25。在 `zcode-cli-rust-history-request-differential.test.ts` 中抓取两个 runtime 同一轮真实请求的 `tools`
（App 默认配置、Windows），逐项比对后发现模型看到的工具面并不相同。工具面决定模型行为，属于功能对齐门槛。

## 实测差异（Node 为准）

| 项目                          | Node                                                                                | Rust（修复前）                                                |
| ----------------------------- | ----------------------------------------------------------------------------------- | ------------------------------------------------------------- |
| Glob / Grep                   | **不暴露**：默认 embedded search 分支（Bash 可用即开启）改由 Bash 的 find/grep      | 暴露                                                          |
| Bash 的 find/grep             | posix / git-bash 会话里注入 prelude：`find`→bfs、`grep`→ugrep、缺 rg 时补 rg 函数   | 无                                                            |
| 工具描述                      | TS 元数据描述（Read 784 字符等）；Bash、EnterPlanMode、Agent 按 embedded 分支取变体 | Read/Write/Edit/Glob/Grep/Bash/TaskOutput/TaskStop 为手写短句 |
| Agent 描述                    | TS 模板内联 profile 列表（Explore 工具按 embedded 分支）                            | 追加一段 Rust 自拟的 “Current profile catalog”                |
| 顺序                          | TS provider 排序                                                                    | Rust 自定顺序                                                 |
| WebFetch                      | 暴露                                                                                | 无（阶段 2）                                                  |
| ReadSessionContext            | 暴露                                                                                | 无（阶段 3）                                                  |
| CronCreate/Delete/List/Update | 暴露                                                                                | 无（与自动任务一并，待用户确定范围）                          |
| 参数 schema                   | —                                                                                   | 语义一致，仅 JSON 键顺序不同（serde_json 排序）               |

## 规则（阶段 1）

- **embedded search 分支**（与 TS `resolveRuntimeEmbeddedSearchEnabled` 一致）：Bash 在当前工具面可用即开启；
  开启时主会话与子代理都不暴露 Glob/Grep。
- **描述**：由 `scripts/generate-zcode-cli-rust-tool-schemas.mjs` 从 TS 生成 embedded 分支下的 provider 描述
  （`crates/tools/src/tool_surface.json`，Agent 模板在 `crates/domain/src/agent_description_template.json`）；Bash 用 `createBashProviderDescription`（默认 120000 / 最大 600000 ms，与 Rust 超时策略一致）。
- **Agent 描述**：生成 TS `buildAgentProviderDescription` 的头尾模板（`dynamicWorkflowEnabled=false`，Rust 不支持工作流），
  Rust 按当前 profile 目录渲染 `- name: description (Tools: …)`；内置 Explore 用 TS embedded 分支的工具文案，
  其余 profile 用自身 tools 去掉 disallowedTools。删除 Rust 自拟的 catalog 段。
- **顺序**：TS `orderProviderVisibleToolContracts`——参考集合内的工具按 `localeCompare` 排序在前（生成资产 `providerOrder`），其余（SendMessage、MCP 等）保持原顺序在后。
- **Bash prelude**：会话 shell 为 posix 或 git-bash 时，把 TS `buildEmbeddedSearchPreludeContent` 同等内容写入
  `<artifacts>/bash-startup/<session>/embedded-search-startup-<sha256前16位>.sh`，命令前追加 `. '<path>'`；
  backend 与 TS `resolveDefaultEmbeddedSearchBackend` 相同：设置了 `ZCODE_EMBEDDED_SEARCH_COMMAND` 时为 internal-cli，
  否则为 native-binaries（`ZCODE_BFS_BINARY` / `ZCODE_UGREP_BINARY` / `ZCODE_RG_BINARY`，由 Host 注入，缺省为命令名）。
  cmd / legacy shell 不注入。

## 已知差异（阶段 1 之后）

- Glob/Grep 不再提供给模型，但仍可执行（旧历史或模型臆造调用不会报 tool_not_found）；TS 未注册时返回 tool_not_found。
- TS 在 posix / git-bash 会话还会先 source shell 初始化快照（用户 profile 快照）；Rust 未移植该快照。
- 参数 schema 仅键顺序不同（serde_json 排序），语义一致，未改。

## 验收

- prelude：生成器以 TS 函数为 oracle 导出 backend × 方言 语料，Rust 逐条逐字比对。
- 工具面：请求差分用例断言两侧工具**名称与顺序**、**描述**逐字一致，参数 schema 语义一致；
  未实现的工具（WebFetch、ReadSessionContext、Cron\*）写成显式白名单，实现后移出。
