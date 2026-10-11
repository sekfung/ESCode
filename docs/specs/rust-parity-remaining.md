# Rust stdio 剩余对齐清单

2026-09-22，目标是替换 App 使用的 stdio runtime。权限只支持 yolo，不做 TUI；默认仍为 TS，Rust 显式选择。用户指定的关键工作包已实现并经过核心验收，不能再把 MCP、Skill、子代理、Goal 和历史操作笼统列为“未实现”。证据见 [核心交付报告](../reports/rust-critical-parity-2026-09-22.md)。

## 本次完成的核心能力

| 能力             | 已实现与验证                                                                                                                                | 剩余边界                                                                       |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------ |
| Session 按需加载 | 启动只读 metadata index、ACK 按 key 查询、单会话激活、8 个/16 MiB 空闲会话 LRU；300 会话故障/驻留测试                                       | 单个超大会话仍整体加载；活跃/有订阅会话按真实需求驻留                          |
| MCP              | 真实 stdio、Streamable HTTP、legacy SSE；工具发现/调用、连接复用、取消、进程回收、失效配置隔离                                              | OAuth、完整插件 options 模板、legacy SSE 自动重连与全部服务器兼容矩阵          |
| Skill            | 用户/项目/启用插件发现；会话冻结 metadata、调用时读正文、冷恢复                                                                             | 全部插件安装/更新/hooks 与主代理记忆系统                                       |
| 子代理           | Agent/Task、SendMessage、TaskOutput/TaskStop；前后台、真实 child Session、profile/model/tools/maxTurns、继承 Skill/MCP、memory 文件、取消树 | 完整 JSONL transcript artifact、子树预算聚合和更多平台验证                     |
| Goal             | 设置/推进/隐藏验证/继续、暂停恢复、队列/后台等待、预算与冷恢复；真实 App verified                                                           | 无效 verifier JSON 保留失败状态，明确区别于 TS 的宽松通过规则                  |
| 重试与编辑       | 原 canonical 意图、附件快照、Goal；同会话历史截断，独立 ACK 凭据、新 epoch 与迟到事件隔离                                                   | 旧历史缺少可证明边界时拒绝；当前 App 产品 UI 不渲染普通 retry 按钮，协议已验收 |
| 分支             | 稳定回复边界、父会话运行中 fork、隔离队列和执行状态、child 与 ACK 原子提交                                                                  | 更多历史导入来源的边界迁移                                                     |
| 文件回退         | Write/Edit 字节/hash/权限 checkpoint、去重 blob、外部修改冲突、恢复 journal、提交失败补偿；App 摘要/diff/编辑并回退                         | 不追踪 Shell 或外部工具任意改文件；保留策略和未引用 blob 回收                  |

2026-09-22 性能更新：首段降低 36%–38%，12 会话空闲 RSS 降低 26.6%，大页查询 312 → 5.3 ms；同机 release 各五次，完整口径及剩余回退见 [性能报告](../reports/rust-performance-2026-09-22.md)。

## 后续优先级

| 优先级   | 后续工作                              | 完成标准                                                                                                                                              |
| -------- | ------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| P1       | 性能长尾与真实工作负载                | 非仓库首段与大历史缓存/分页优化已完成；继续验证真实 TS/供应商、大活跃历史、流式稳定阶段 RPC 长尾，不能凭语言宣称更快                                  |
| P1       | MCP 鉴权与真实服务器兼容              | OAuth、插件配置插值、断线/凭据变更、HTTP/SSE 大结果边界；按在用服务器逐一实测                                                                         |
| P1       | 历史与文件恢复剩余边界                | 旧 TS/Rust 边界迁移、跨子代理历史恢复、Shell/外部修改明确的产品范围；跨平台故障恢复                                                                   |
| P1       | 大历史与磁盘容量                      | 单会话分页/按需投影、已提交备份跨 workspace 去重和保留策略、未引用附件/checkpoint GC；大库实测                                                        |
| P1       | 主代理上下文扩展                      | 记忆索引/提取、output style、自定义 system、插件 hooks，与已有 Skill/profile 区分                                                                     |
| P1       | 附件与工具细节                        | 大图缩放、媒体 Read、完整文本预览、音频/部分视频、Edit 宽松匹配、Web 工具及工具装配开关                                                               |
| P1       | App 生命周期完整矩阵                  | TS/Rust 切换、带 Plan 导入、远端/手机双语义、账号供应商多协议回归                                                                                     |
| P2       | 工作流、自动任务、OffPeak、浏览器工具 | 真实执行与持久化投影；App workflowRuns 目前仍会报告 unsupported，不能以空结果伪装完成                                                                 |
| P1       | App 侧 local TTFT 与请求级遥测        | Rust 不发 `ModelNetworkStatus`、不产 `frame.ttft`，App 侧针对 Rust 会话的 TTFT/请求遥测缺失；范围与实现要点见 [性能文档](rust-runtime-performance.md) |
| 发布门槛 | 三平台发行与回退                      | macOS/Windows/Linux 原生进程、打包升级、历史兼容；TS/Rust release 与真实供应商对比                                                                    |

已确认的基础链路包括账号模型、三种模型协议、AskUserQuestion、Todo、shared context handover、本地文本附件、取消/队列/冷恢复。相关报告保留在 `docs/reports/`。本清单区分功能核心完成、残余兼容差分和发布门槛，不宣称全量 TS 替换已经验收。

## App 协议方法级 diff（2026-09-30）

做法：把 App 客户端（`packages/services/src/escode-agent/*`）引用的 `escodeProtocolMethods.*` 与 Rust engine 的
method 表逐条用 `rg --fixed-strings` 对比，对命中"App 有调用、Rust 无实现"的项再做真机探针。

- **已修**：`session/resume`（探针实测 Node 正常、Rust 回 `-32601 Unsupported method`；实现与验收见
  [rust-session-loading.md](rust-session-loading.md)）。
- **已修（2026-10-03 复查）**：上次 diff 按 V4 主路径判断漏掉了 legacy facade——普通 `session/create`（Rust 误当共享
  导入拒绝）、`session/send`（带附件）、`session/setModel`、`session/setThoughtLevel`、`session/setMode`、`session/close`
  在 Rust 上全部失败，触达定时/闲时任务首跑、Bots 附件与 `/model`、恢复历史任务的档位重放、框选副屏关闭等。已翻译为
  V4 命令实现并与 Node 差分一致（[rust-legacy-session-methods.md](rust-legacy-session-methods.md)）。`session/compact`、
  `session/goal` 无 App 调用点，不实现。
- **仍缺**：`session/create` 的 `importedHistory.source = "claudeCode"`（Claude Code 历史导入与
  `importedClaudeHistoryRepair` 修复），Rust 只接受 `sharedContext`，需另立工作包。
- **仍缺**（按当前行为判断的重要性排序）：
  - 插件读面与开关：`plugins/list`（含 components 与发现层诊断）、`plugins/setEnabled`、`plugins/overview`、
    `plugins/referenceCatalog(WithCategory)` **已实现**，各有 App 差分（见 [rust-plugins.md](rust-plugins.md)）。
    仍缺：选项面（`userConfig`/`configuredOptions`/`optionSources`、`plugins/configure`/`resetConfig`）、
    hook 发现与 `hookDetails`，以及市场写面（`plugins/install`/`uninstall`/`update`/`restoreBuiltin`、
    `plugins/marketplace*`、`resolveSuggestedReference`）——App 插件页的安装/配置操作对 Rust 仍不可用。
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
