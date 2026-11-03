# Rust stdio 剩余对齐清单

2026-09-22，目标是替换 App 使用的 stdio runtime。当时的边界已不适用：权限模式现宣告 `yolo/build/edit/guarded`（`apps/escode-cli-rust/crates/core/src/app/queries.rs:31-35`），桌面默认 runtime 也已切换为 Rust（commit `7950284d`，见 [rust-cli-architecture.md](rust-cli-architecture.md)）；仍不做 TUI。用户指定的关键工作包已实现并经过核心验收，不能再把 MCP、Skill、子代理、Goal 和历史操作笼统列为“未实现”。证据见 [核心交付报告](../reports/rust-critical-parity-2026-09-22.md)。

## 本次完成的核心能力

| 能力             | 已实现与验证                                                                                                                                | 剩余边界                                                                                                                                                                                                                                                                               |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Session 按需加载 | 启动只读 metadata index、ACK 按 key 查询、单会话激活、8 个/16 MiB 空闲会话 LRU；300 会话故障/驻留测试                                       | 单个超大会话仍整体加载；活跃/有订阅会话按真实需求驻留                                                                                                                                                                                                                                  |
| MCP              | 真实 stdio、Streamable HTTP、legacy SSE；工具发现/调用、连接复用、取消、进程回收、失效配置隔离                                              | 全部服务器兼容矩阵；legacy SSE 自动重连已完成（2026-10-31，见 [rust-mcp-parity.md](rust-mcp-parity.md)）；OAuth（授权码 + PKCE、client_credentials）与插件配置插值已完成（[rust-mcp-oauth.md](rust-mcp-oauth.md)）                                                                     |
| Skill            | 用户/项目/启用插件发现；会话冻结 metadata、调用时读正文、冷恢复                                                                             | 插件安装/更新、hooks 与主代理记忆已完成（[rust-plugin-marketplace-write.md](rust-plugin-marketplace-write.md)、[rust-hooks.md](rust-hooks.md)、[rust-project-memory.md](rust-project-memory.md)）；Dream / Semantic Recall 不跟进（见 [rust-v3.15.1-drift.md](rust-v3.15.1-drift.md)） |
| 子代理           | Agent/Task、SendMessage、TaskOutput/TaskStop；前后台、真实 child Session、profile/model/tools/maxTurns、继承 Skill/MCP、memory 文件、取消树 | 产物文件集与布局已对齐 Node（2026-11-03）：`<storage>/cli/agents/<parent>/<agent>/{output.txt,task.output,metadata.json}`；完成/失败/停止与 Node 差分已覆盖；子树预算聚合和更多平台验证                                                                                                |
| Goal             | 设置/推进/隐藏验证/继续、暂停恢复、队列/后台等待、预算与冷恢复；真实 App verified                                                           | 无效 verifier JSON 保留失败状态，明确区别于 TS 的宽松通过规则                                                                                                                                                                                                                          |
| 重试与编辑       | 原 canonical 意图、附件快照、Goal；同会话历史截断，独立 ACK 凭据、新 epoch 与迟到事件隔离                                                   | 旧历史缺少可证明边界时拒绝；当前 App 产品 UI 不渲染普通 retry 按钮，协议已验收                                                                                                                                                                                                         |
| 分支             | 稳定回复边界、父会话运行中 fork、隔离队列和执行状态、child 与 ACK 原子提交                                                                  | 更多历史导入来源的边界迁移                                                                                                                                                                                                                                                             |
| 文件回退         | Write/Edit 字节/hash/权限 checkpoint、去重 blob、外部修改冲突、恢复 journal、提交失败补偿；App 摘要/diff/编辑并回退                         | 不追踪 Shell 或外部工具任意改文件；保留策略和未引用 blob 回收                                                                                                                                                                                                                          |

子代理产物：Node 与 Rust 均写 `<storage>/cli/agents/<parent>/<agent>/{output.txt,task.output,metadata.json}`；
`escode-cli-rust-subagents.test.ts` 覆盖完成/失败/停止，`escode-cli-rust-subagent-artifacts-differential.test.ts`
做 Node/Rust 同输入对照。Node 已不写 `transcript.jsonl`，见
[07-subagent.md](../../apps/escode-cli/docs/design/v2/tool/07-subagent.md) 与 `subagent-explore.test.ts` 的 ENOENT 断言。

2026-09-22 性能更新：首段降低 36%–38%，12 会话空闲 RSS 降低 26.6%，大页查询 312 → 5.3 ms；同机 release 各五次，完整口径及剩余回退见 [性能报告](../reports/rust-performance-2026-09-22.md)。

## 后续优先级

| 优先级   | 后续工作                       | 完成标准                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| -------- | ------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| P1       | 性能长尾与真实工作负载         | 非仓库首段与大历史缓存/分页优化已完成。大活跃历史离线基线已补齐（2026-10-31，`escode-cli-rust-long-history-bench.test.ts` 新增 `ESCODE_BENCH_FIXTURE=1`；300 轮 / 5 次交错：Node 二次打开 p50 2073ms、Rust 358ms，首次打开含一次性导入 2277/685ms；2000 轮 / 4.1 MiB：Node 二次打开 2343ms、Rust 623ms，首次导入 7643ms；rows 均返回 200 行分页；[报告](../reports/rust-long-history-fixture-2026-10-31.md)）。三平台原生 20k 轮已由 CI 手动作业补齐（2026-11-02，run 41446166592：Rust 二次打开 Linux/macOS/Windows 509/263/624ms，Node 3845/2750/4509ms；rows 200 行、schemaErrors 0）；真实供应商与流式稳定阶段 RPC 长尾仍待验证，不能用 mock 数据替代。                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| P1       | MCP 鉴权与真实服务器兼容       | 凭据变更、按在用服务器逐一实测。普通 MCP 大文本结果已对齐 Node 50*000 字节 resultBudget（2026-10-31，`escode-cli-rust-mcp-large-result-differential.test.ts`：50k 边界、ASCII/中文、两种协议；node_repl 的 artifact 预算已对齐（2026-10-31，`escode-cli-rust-node-repl.test.ts`「large js output」：>64 KiB 落盘 + `<persisted-output>` 信封；[rust-mcp-parity.md](rust-mcp-parity.md)）；官方 CUA 的 256 KiB 预算已按私有 authority 门对齐（2026-10-31，`crates/domain/src/mcp_cua.rs` 的 `official_authority_verified` + `ResultBudget`；缺失/半组/错误 authority、非 stdio 与 `node_repl` 均 fail-closed；当前产品该分支因 CUA 形 stdio server 退役而不可达）。legacy SSE 断线自动重连已完成（2026-10-31，`crates/tools/src/mcp_sse.rs`；差分 `escode-cli-rust-mcp.test.ts`「legacy SSE reconnects」，见 [rust-mcp-parity.md](rust-mcp-parity.md)）。OAuth 与插件配置插值已完成（`crates/tools/src/mcp_oauth*\*.rs`；差分 `escode-cli-rust-mcp-oauth.test.ts`、`escode-cli-rust-mcp-official-auth.test.ts`，见 [rust-mcp-oauth.md](rust-mcp-oauth.md)）                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| P1       | 历史与文件恢复剩余边界         | 旧 TS/Rust 边界迁移、跨子代理历史恢复已离线复跑（2026-10-31）：`escode-cli-rust-migration.test.ts`（真实 TS 存储导入、workspace identity、compact 保留尾部）、`escode-cli-rust-subagents.test.ts`（冷历史子会话取消、后台续聊、父文件回退包含子写入）；Shell/外部 MCP 任意写入的不可回退范围见 [rust-history-actions.md](rust-history-actions.md)。TS/Rust 热切换的安装包验证与跨平台故障恢复仍待 CI/对应平台环境。                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| P1       | 大历史与磁盘容量               | 单会话分页/按需投影、已提交备份跨 workspace 去重和保留策略、未引用附件/checkpoint GC；2026-11-02 补 20,000 轮 / 39 MiB 与 50,000 轮 / 98 MiB release 实测：Node 首次/二次打开 5303/5082ms、Rust 253246/742ms，50k 下 Node 10490/10256ms、Rust 1654037/1704ms，rows 200 行 5/6ms；修复两处 O(rows²) 投影扫描后 20k/50k 首次打开降至 4.1s/9.7s，二次打开 814/1792ms（[报告](../reports/rust-long-history-fixture-2026-10-31.md)）。100k 轮 Node 首次/二次打开 18990/18379ms、Rust 18475/3059ms，rows 200 行 6-7ms；首次导入相对 50k 为 1.9 倍（轮数 2.0 倍），保持近线性。100k 源库 196 MiB 导入后临时目录 535 MiB（Rust 库 339 MiB + 保留的 TS 备份 197 MiB），成功备份保留策略仍需产品确认。真实用户小库只读演练已通过（1 会话/21 轮，行哈希/附件/todo 全匹配，原库未改）；三平台原生 20k 轮已由 CI 手动作业补齐（run 41446166592，rows 200 行、schemaErrors 0）；真实安装库、真实用户大库与磁盘容量/保留策略实测仍待补                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| P1       | 主代理上下文扩展               | 记忆索引/提取与插件 hooks 已完成（`crates/core/src/app/memory_run.rs`、`crates/core/src/app/memory_extraction.rs`、`crates/tools/src/hooks.rs`；差分 `escode-cli-rust-memory-differential.test.ts`、`escode-cli-rust-hooks-turn.test.ts` 等，见 [rust-project-memory.md](rust-project-memory.md)、[rust-hooks.md](rust-hooks.md)）；output style / 自定义 system 在 App 链路无入口，无需实现（[rust-release-rollback.md](rust-release-rollback.md)）                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| P1       | 附件与工具细节                 | 大图缩放、媒体 Read（图片/PDF/视频）、Edit 宽松匹配、Web 工具（WebFetch/WebSearch）与工具装配开关已完成（差分 `escode-cli-rust-media-read-differential.test.ts`、`escode-cli-rust-tool-parity.test.ts`、`escode-cli-rust-webfetch-differential.test.ts`、`escode-cli-rust-websearch-differential.test.ts`；见 [rust-media-read.md](rust-media-read.md)、[rust-edit-matching.md](rust-edit-matching.md)、[rust-webfetch.md](rust-webfetch.md)、[rust-websearch.md](rust-websearch.md)、[rust-tool-surface.md](rust-tool-surface.md)）；完整 Read 文本预览差分已对齐（2026-10-31，`escode-cli-rust-read-text-parity.test.ts`：256 KiB/25000 token 边界、offset/limit、空文件、BOM、UTF-16LE、GBK、重复读取）；本地路径附件大小/分类已对齐（2026-10-31，`escode-cli-rust-attachment-limits.test.ts`：>20 MiB 文本预览、>20 MiB 图片/PDF 与 >30 MiB 视频路径引用、21 MiB 视频内联、音频二进制路径引用、BOM/GBK、删除源文件后的历史重放；[rust-prompt-attachments.md](rust-prompt-attachments.md)）；ToolSearch local/native 已端到端覆盖（2026-10-31，`escode-cli-rust-tool-search-e2e.test.ts`：local 结果展开 `<functions>` schema、DeferredToolCall 执行/拒绝、off 基线），目录增删/恢复公告已按上游 `upstream/main` 的 `exposure.ts` / `history.ts` / `tool-search-announcement-*.test.ts` 实现（2026-11-01：`crates/domain/src/tool_search_announcement.rs`、core 步骤落盘与投影、`escode-cli-rust-tool-search-e2e.test.ts`；本 fork 的 Node CLI 仍固定 `toolSearchMode=off`，帧级差分待上游模块同步，见 [rust-tool-search.md](rust-tool-search.md) §5.1） |
| P1       | App 生命周期完整矩阵           | TS/Rust 切换、带 Plan 导入、远端/手机双语义、账号供应商多协议回归。离线已复跑（2026-10-31）：`escode-cli-rust-migration-boundaries.test.ts`（带 Plan 导入）、`escode-cli-rust-resume-differential.test.ts`（`desktop-continuous` + `web-remote-replayable`）、`escode-cli-rust-account-host.test.ts`、`escode-cli-rust-protocols.test.ts`，11 + 12 用例通过；TS/Rust 切换仍由 `escode-cli-rust-installed.test.ts` 在安装包 CI 环境验证；本机已用仓库 bundle 演练（非安装包，见下表后注）。                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| 已完成   | 工作流、自动任务、浏览器工具   | 工作流只读查询经工作流宿主应答（`crates/core/src/app/engine_requests.rs:125-133`；差分 `escode-cli-rust-workflow-queries.test.ts`）；Cron 有原生工具与差分（`crates/core/src/app/cron_tool.rs`；`escode-cli-rust-cron-differential.test.ts`）；浏览器工具经内置 node_repl 与浏览器经纪（`crates/tools/src/mcp_node_repl.rs`、`crates/tools/src/browser_broker.rs`；差分 `escode-cli-rust-node-repl.test.ts`，见 [rust-browser-use.md](rust-browser-use.md)、[rust-cron.md](rust-cron.md)）                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| 已删除   | App 侧 local TTFT 与请求级遥测 | 2026-10-21 随遥测功能删除；Rust/Node 只保留内部 `v4/telemetry/event` 会话活动事实，不再产生 `frame.ttft`/`ttftRelated` 或 OTLP 导出                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| 发布门槛 | 三平台发行与回退               | macOS/Windows/Linux 原生进程、打包升级、历史兼容；TS/Rust release 与真实供应商对比                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |

2026-11-03 Rust-only 发行门禁完成：`release` run `41491160036`（`dev` @ `053f086a`，`rust_only=true`）的
8 个 runtime、4 个 desktop、4 个 installed 作业全绿；四平台真实安装包均执行 Rust Host、Rust-only 启动与
安装包会话门禁，Node → Rust → Node runtime 演练按设计跳过。随后 `rust-version-drill` run `41503492904`
（`dev` @ `83ecf28c`）完成 base `v3.15.2` → head → 降级的四平台 Rust-only 演练，`app.asar` 哈希按预期
变化并恢复、冻结设置保持不变。因此「Rust-only 三平台发行门禁」与「Rust-only 升级/降级演练」不再是待办；
删除 TS CLI、本地 Node Host 回退与 `ESCODE_AGENT_SERVER_RUNTIME=node` 仍需 P1 残余项收敛和单独确认。

2026-11-04 已删除 `ESCODE_RUST_HOST=0` 本地 Node Host 回退（S6）：`desktopWindowLifecycle.ts` 的
Node 分支、`desktopHostProcess.ts` 的 spawn/dispose 进程管理、`isRustHostEnabled` 与
`windowHostProcessMap` 装配一并移除；安装包演练改为 Rust Host 重启三次并校验冻结设置，
`bench-escode-app*.mjs` 改为 Rust Host 下的 Node/Rust/embedded runtime 对比，
E2E 不再透传该开关，`ensure-local-runtime-assets.mjs` 只保留 `ESCODE_RUST_HOST_BINARY` 覆盖。
同时删除仅由 Node Host 驱动的 Desktop 侧 cron scheduler（automation 调度由 Rust Host 常驻循环承担）、
`BroadcastHub` / `TaskRealtimeBus` 与 `cua-pip-session` focus 转发（占位包恒 `enabled:false`，Rust Host
不服务该域，见 `rust-u2-host-scope-boundary.md` §2.2）。`desktopContextPromptRollout` 的裁决仍由 Main
写入 Rust Host `runtimeEnv`，但 Rust runtime 尚未消费该键，属独立 P2 迁移项，当前不改变行为。

2026-11-03 流式长尾指标补齐：`escode-cli-rust-real-provider-bench.test.ts` 现在同时输出本轮新文本
`row.delta` 相邻到达间隔的 p50/p95/p99/max，覆盖首段与完成时间看不到的流式稳定阶段停顿。该指标仍需
设置 `ESCODE_BENCH_PROVIDER_ID` 在真实供应商环境执行；本地无供应商配置，未据此宣称 P1 已完成。

2026-10-31 MCP 凭据变更补充：`escode-cli-rust-mcp.test.ts`「MCP credential change reconnects with the rotated env」
在同一 server 名下把 `MCP_PROBE` 从 `scoped` 改为 `rotated`，Node/Rust 的 `mcp/list` 都重连，新 session 工具读到
`rotated`；另有「MCP HTTP credential change reconnects with the rotated header」覆盖 `X-Fixture` header 轮换。
连接身份按完整配置区分的规则见 [rust-mcp-parity.md](rust-mcp-parity.md)。P1 行里的“按在用服务器逐一实测”
仍需可访问的第三方 MCP 服务器与凭据，未据此宣称完成。

2026-10-31 大库补充：同一长历史用例以 10000 轮 / 18.65 MiB（只测打开与分页）跑通，
Node 首次/二次打开 3898/3678ms、Rust 222148/1748ms，`rows` 200 行 5/22ms；10k 轮超过 mock provider
上下文窗口，未测下一轮。release 二进制同一库为 Node 3890/3762ms、Rust 61838/409ms，`rows` 6/5ms；
真实安装库仍待测；20k/50k/100k 轮合成库已在本轮补齐，详见
[大活跃历史离线基线](../reports/rust-long-history-fixture-2026-10-31.md)。

2026-11-01 ToolSearch 补充：local 结果在 `<functions>` 定义块后追加上游
`localToolsLoadedReminder` / `localToolsAlreadyLoadedReminder` 文案（同批并行搜索按批次开始时的
已发现快照判定“已加载”）；成功命中名同时写入结果消息的 `_escode_tool_search_discovered`，
未压缩冷恢复也能重建已发现集合。`escode-cli-rust-tool-search-e2e.test.ts` 覆盖
“首次加载提醒”“再次搜索已加载提醒”“未压缩冷恢复常驻”。

2026-11-01 ToolSearch 准入补充：对齐上游 `resolveDeferredToolCall`——目标是否已发现不再是
DeferredToolCall 的准入条件，未发现时照常解包并按注册表 schema 校验；校验失败追加
`UNDISCOVERED_TOOL_SCHEMA_EXPLANATION`（退化原因 + `select:<name>` + 当前 schema + local 的
DeferredToolCall 提示）。入参校验的 schema 来源同时从“模型可见面”收口到“会话注册表快照”，
隐藏但已注册的工具（local 未发现的延迟工具、Cron/WebSearch）也校验。E2E 覆盖“未发现直接执行”
与“非法入参回灌纠错、已发现后不再追加”，见 [rust-tool-search.md](rust-tool-search.md) §5.3。
控制工具的模型可见描述与 schema 同时对齐上游 `search/description.ts` 与 `tool-search.ts`
（`Result format` 段、local 的 TodoWrite 示例、`max_results.default = 5`、DeferredToolCall 的
`z.record` 入参），只影响请求文本，不改执行语义。

2026-10-31 本地 bundle 切换演练：用仓库 `escode.cjs` 与 release Rust 二进制模拟包内
`resources/glm` 路径，运行 `escode-cli-rust-installed.test.ts`，Node → Rust → Node 在同一数据目录
完成写入、导入读取和回退续聊（1 例通过，TS 库逐字节不变）。它不等同于真实安装包/CI 演练，
Windows/macOS 仍待对应 runner。

2026-10-31 冻结 golden 门禁复跑：`node scripts/diff-escode-cli-rust-parity.mjs --golden --strict`
只启动 Rust，Linux x64 退出码 0；frames 10 条已批准 / 0 open / 0 欠账 / 0 新差异，storageProgress 1 条、
modelRequests 1 条为已批准，workspaceFiles 0。CI 已使用 `--golden`，并在平台欠账基线为空时追加
`--strict`（2026-11-01 起三平台均已启用，见下条与 [rust-unified-host.md](rust-unified-host.md) §4.6）。

2026-11-01 平台归一化与响应面拆分：冻结 golden 的已批准差异按双侧 SHA-256 钉住，但提示词字节数
（`"chars"`）与 `<ROOT>` 后的路径分隔符是平台事实，导致 macOS/Windows 上同一已批准差异因 sha 不同被判为
新差异；JSON-RPC 响应又与通知帧混排，Windows 录制的 frame→ack 与 Rust 固定的 ack→frame 形成顺序竞态。
差分层新增 `prompt-char-counts`、`workspace-path-separator` 两条归一化规则，并把响应拆到独立
`rpcResponses` 面（按出现顺序严格，相对通知帧的位置不进契约），见
[rust-unified-host.md](rust-unified-host.md) §4.2/§4.6。`--golden --strict` 在 Linux 通过（frames 7、
rpcResponses 3、storageProgress 1、modelRequests 1 全部已批准，0 欠账）。三平台 CI 复验（run
`41270183483`，commit `57179fa0`）：`bench (ubuntu-latest / macos-latest / windows-latest)` 全部 success，
其中 macOS/Windows 均以 `--golden --strict` 运行并判定 parity（同样 0 欠账、0 新差异），跨平台 golden
strict 门禁就此转绿。

当前门禁形态（2026-11-03 复核并收紧默认模式）：
`packages/services/tests/escode-cli-rust-golden.ts` 的 `nodeOracle` 只读写冻结 golden，未录制时缺文件直接失败，
不会启动 Node；`scripts/diff-escode-cli-rust-parity.mjs` 不带模式参数时按 `--golden` 运行，只有显式
`--record` 或 `--live-node` 才启动 Node，避免删除 Node 后默认差分悄悄回退到实时 oracle。
`.github/workflows/rust-runtime-ci.yml` 的平台解析步骤要求 golden 存在，并在
`scripts/diff-escode-cli-rust-parity.baseline.<platform>.json` 为空时自动追加 `--strict`。三份平台欠账基线
（`darwin-arm64` / `linux-x64` / `win32-x64` 的 `session-baseline`）均为空数组，因此三平台 CI 实际都跑
`--golden --strict`。删除 Node runtime 的前置条件「差分套件完全依赖冻结 golden，不再启动 Node oracle，
并切到 `--strict`」据此已满足；录制新 golden 仍需 `record cli goldens` workflow，不在 CI 里回退到实时 Node。

2026-11-04 本地复核（Linux x64，release 二进制）：`node scripts/diff-escode-cli-rust-parity.mjs --golden --strict`
判定 parity：frames 7、rpcResponses 3、storageProgress 1、modelRequests 1 全部已批准，0 open / 0 欠账 /
0 新差异；同一场景改用 `--live-node`（当前 `apps/escode-cli/packages/cli/dist/escode.cjs`）同样 0 新差异，
说明冻结 golden 与当前 Node 产物一致，本地不需要回退实时 oracle。同日全量 Rust parity 套件（170 个测试文件）
在 CI 同款环境（`ESCODE_TEST_REQUEST_MS=60000`、`--test-concurrency=1`）下 425 通过 / 0 失败 / 14 跳过。

2026-11-01 CI 跨平台 test 分片三处失败的处置（run `41270183483`，commit `57179fa0`）：

1. plugins hooks / diagnostics 的 `plugin_root_not_found`（`.../cache/zcode-plugins-official/rea/0.1.0`）：
   rea 是可选随包插件，seed 未覆盖时 catalog 里的 `cachePath` 不存在。Node `scanOfficialCache` 不查
   存在性、由 loader 产出该诊断（`plugins/list` 语义），Rust 原本一致；hook 用例按 `pluginId` 过滤官方
   插件观察，而该诊断没有 `pluginId`，且诊断用例的 rea 过滤只匹配 `/`，Windows 反斜杠漏过滤后把环境
   差异写进了平台 golden。修复：两处过滤先归一分隔符再匹配（`plugins-hooks` / `plugins-diagnostics`），
   win32 诊断 golden 去掉被契约过滤的 rea 条目；Rust 发现层保持 Node 语义，
   `extension_plugins_tests.rs` 断言缺失根必须产出该诊断。
2. `escode-cli-rust-create-workflow.test.ts`（Windows）期望 `.escode/workflow-drafts/...`、Rust 输出
   `.escode\workflow-drafts\...`。Node 在 Windows 录制时确实写反斜杠（`472a96ec` 的原始
   `create-workflow.observe.win32-x64.json`），`daaa2043` 手工同步 golden 时把它覆盖成了 Linux 值；
   恢复 Windows golden 的平台事实，Rust 不动（`path.relative` 语义）。
3. `escode-cli-rust-node-repl.test.ts`（macOS）`<persisted-output>` 里的产物路径多出 `/private`：
   Rust `resolve_workspace` 对 `--data-dir` 做 realpath（`/var` → `/private/var`），Node 的 storage 根
   只做 `path.resolve`。改为词法归一（复用 `escode_cli_plugins::lexical_path`），回归
   `runtime::tests::data_dir_keeps_the_requested_spelling_like_node_path_resolve`；
   状态键与文件比较仍用 realpath。

已确认的基础链路包括账号模型、三种模型协议、AskUserQuestion、Todo、shared context handover、本地文本附件、取消/队列/冷恢复。相关报告保留在 `docs/reports/`。本清单区分功能核心完成、残余兼容差分和发布门槛，不宣称全量 TS 替换已经验收。

## App 协议方法级 diff（2026-09-30）

做法：把 App 客户端（`packages/services/src/escode-agent/*`）引用的 `escodeProtocolMethods.*` 与 Rust engine 的
method 表逐条用 `rg --fixed-strings` 对比，对命中"App 有调用、Rust 无实现"的项再做真机探针。

- **已修**：`session/resume`（探针实测 Node 正常、Rust 回 `-32601 Unsupported method`；实现与验收见
  [rust-session-loading.md](rust-session-loading.md)）。
- **已修（2026-10-03 复查）**：上次 diff 按 V4 主路径判断漏掉了 legacy facade——普通 `session/create`（Rust 误当共享
  导入拒绝）、`session/send`（带附件）、`session/setModel`、`session/setThoughtLevel`、`session/setMode`、`session/close`
  在 Rust 上全部失败，触达定时/闲时任务首跑、恢复历史任务的档位重放、框选副屏关闭等。已翻译为
  V4 命令实现并与 Node 差分一致（[rust-legacy-session-methods.md](rust-legacy-session-methods.md)）。`session/compact`、
  `session/goal` 无 App 调用点，不实现。
- **已实现基础导入（2026-10-18，见 [rust-legacy-session-methods.md](rust-legacy-session-methods.md)）**：
  `session/create` 的 `importedHistory.source = "claudeCode"` 建会话、消息投影与持久化。
- **已修（2026-10-31）**：同 `sessionId` 重导入的历史替换。Rust 按 `_escode_import_source` 标记（对齐 TS
  `migrationSource`，旧消息退回 `msg_import_*` 前缀）删除旧导入行/消息，保留用户后续续聊，把新导入段追加到末尾
  （与 Node `sessionStore.messages()` 的 `sequence` 顺序一致），并按 `historyRewrite` 全量重写、换 epoch 发 recovery
  快照。差分：`packages/services/tests/escode-cli-rust-session-import-repair.test.ts`。
- **仍缺**（按当前行为判断的重要性排序）：
  - 插件读面与开关：`plugins/list`（含 components 与发现层诊断）、`plugins/setEnabled`、`plugins/overview`、
    `plugins/referenceCatalog(WithCategory)` **已实现**，各有 App 差分（见 [rust-plugins.md](rust-plugins.md)）。
    选项面（`userConfig`/`configuredOptions`/`optionSources`、`plugins/configure`/`resetConfig`）与市场写面
    （`plugins/install`/`uninstall`/`update`/`restoreBuiltin`、`plugins/marketplace*`、`resolveSuggestedReference`）
    **已实现**（`apps/escode-cli-rust/crates/core/src/app/engine_requests.rs:152-180`、
    `apps/escode-cli-rust/crates/core/src/app/plugin_jobs.rs`；差分 `escode-cli-rust-plugins-options.test.ts`、
    `escode-cli-rust-plugins-install.test.ts`、`escode-cli-rust-plugins-uninstall.test.ts`、
    `escode-cli-rust-plugins-marketplace.test.ts`、`escode-cli-rust-plugins-suggested.test.ts`；见
    [rust-plugins.md](rust-plugins.md)、[rust-plugin-marketplace-write.md](rust-plugin-marketplace-write.md)）。
    hook 源发现、`hookDetails` 与 components 的 hook 分组已实现（2026-10-31，见
    [rust-plugins.md](rust-plugins.md) 第 4b 期），App 插件页的安装/配置操作现已可用。
  - `session/messages`：**已实现**（`afterMessageId`/`limit` 分页语义与 TS 一致，见
    [rust-session-loading.md](rust-session-loading.md)）；但该方法的 App 调用方 `readSessionMessages`
    当前没有活跃 UI 消费者，且 Node 返回的 legacy 形状过不了 App 自己的
    `escodeSessionMessagesResultSchema`——要不要保留这条 legacy 面需要产品决定。
  - `workspace/updateModelIoPreferences`：**已实现**，且补上了它背后真正的缺口——Rust 之前完全不写 model-io，App「模型调用轨迹」侧栏对 Rust 会话为空；见 [rust-model-io.md](rust-model-io.md)。
  - `computer-use/operation-event`：**已实现**（2026-10-03，由遥测事实派生 turn-started / tool-scheduled（含 CUA
    引导语句判定）/ tool-started / turn-completed|failed；差分 `escode-cli-rust-cua-operation-event.test.ts`）。
  - `v4/cua/permission-observation`：已核对为**当前产品不可达**——TS 只对 `kind: "cua"` 展示（`mcp__computer_use__*`
    工具）的 `request_access` 结果发出，而 TS 与 Rust 都把除 `node_repl` 外的 CUA 形 MCP 服务器退役（`isRetiredCuaMcpServer`），
    `node_repl` 的 CUA 走 `node_repl_images` 展示（Rust 已对齐）。Rust 不实现，若 TS 恢复 computer-use MCP 再补。
  - 已核对为**无活跃 UI 调用点**（Rust 不实现，作为 legacy 面保留在 TS 侧）：
    `session/events`、`session/debug`、`session/subscribe`（V4 已用 `v4/conversation/subscribe` 取代）、
    `workspace/hooks/trustGrant` 已随 hooks H2 实现（见 [rust-hooks.md](rust-hooks.md)）。`plugins/referenceCatalog*` 同样没有活跃 UI
    调用点，但 TS 仍提供，Rust 已一并实现（见上）。

2026-11-01 三平台 CLI 差分门禁：`rust-runtime-ci` run `41282629407`（commit `f6f4b10f`）的
`test` 9 个分片（ubuntu/macOS/windows × 1..3）全绿，`bench` 三平台 strict 全绿。同一轮补了两个
Windows 平台 golden（`attachment-limits.*.win32-x64.json` 9 个、`read-text-parity.cases.win32-x64.json`），
由 `record cli goldens` run `41279354207` 在 windows-latest 用 `ESCODE_GOLDEN_PLATFORM=1` 录制，
未手抄 Linux 值（规则见 [rust-unified-host.md](rust-unified-host.md)）。这不代表 P1 的
「真实供应商 / 第三方 MCP / 安装包演练」已完成。
