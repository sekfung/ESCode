# Circuit IR 由 Host 服务持有

- 状态：Accepted
- 日期：2026-10-12

## Context

电路识别与分析（见 [docs/circuit](../circuit/README.md)）需要让 Agent 和电路面板读写同一份 Circuit IR：Agent 负责解析与修正，用户在面板里修正解析结果。我们考虑过两种所有权方案：

- **以文件为事实来源**：IR 和人工修正分别写进 workspace 里的两个文件，各有一个写入方；面板通过文件服务监听变化。纯 CLI、远程 workspace 和手机端都能直接使用，不需要新增 Host 服务和协议。缺点是 Agent 可以用通用的 Edit 工具直接改文件，多出一条绕过校验的写入路径；而且 revision、冲突检测、置信度只能由服务端写入，这些约束只能靠约定来维持。
- **Host 服务持有**：仿照 `serial`，由 `CircuitService` 持有 IR，Agent 通过 MCP 和 broker 访问，面板通过 hooks 访问。

## Decision

采用 Host 服务持有。`packages/services/src/circuit/` 中的 `CircuitService` 是 Circuit IR 的唯一所有者：

```text
Agent ─MCP─▶ circuit-plugin ─broker─▶ CircuitService ◀─hooks─ 电路面板
                                           │
                                  AppData/circuits/<identityKey>/…
```

- 所有写入都必须带 `baseRevision`，版本落后就拒绝。持久化完成后才发出 `circuit.changed{revision}` 事件。
- IR 存放在 Host 应用数据目录，按 `workspaceIdentity?.trim() || workspacePath` 和源文件路径区分。导出到 workspace 的只是只读快照，服务永远不读回。
- 置信度只能由服务端写入。

## Consequences

- IR 只有一条写入路径，校验、revision 和置信度规则由服务强制执行。
- 纯 CLI/TUI 下没有 broker，电路工具返回 `unavailable`；不提供 CLI 内的降级引擎，以免出现第二个所有者。
- 需要新增 shared schema、协议字段、服务、broker 端点、远程服务访问和 UI hooks，实现成本高于以文件为事实来源的方案。
- 手机端和 Web 客户端不在 P1 范围内，`::circuit` 引用在这两个环境只显示静态降级卡片。将来如果要支持，需要另外补充 `web-remote-replayable` 的"快照 + revision"恢复语义。
- 如果 IR 需要纳入版本控制，只能通过 `circuit_export` 导出快照。
