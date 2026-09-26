# Rust 会话行投影字段对齐

2026-09-26。承接 rust-release-rollback.md「仍存在、未纳入断言的差异」。以 TS
`bootstrap/src/zcode-protocol-v4/{projection-rows,product-projection}.ts` 为 oracle。
差分实测：一次 Read 工具调用的完整 turn，逐行比较两侧的字段集合。

## 差异与处理

| 行         | Node 有、Rust 缺        | UI 消费                                                                                  | 处理                                                                                                       |
| ---------- | ----------------------- | ---------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- |
| 所有行     | `visibility: "visible"` | 分享投影透传                                                                             | 行基础字段统一带上                                                                                         |
| turnHeader | `executionKind`         | `conversationTurnWorkSegments`：`controlOnly` 不显示「已工作」；`agent` 按 duration 判定 | 普通轮为 `agent`；`/goal` 的可见 query 轮为 `controlOnly`（TS 注释：真实执行属于随后的 goalContinuation）  |
| turnHeader | `activeMs`              | 「已工作 N 秒」优先读取                                                                  | 普通轮收口时写入 `endedAt - startedAt`（TS 取 runtime 的 turn duration，两者量级一致）；controlOnly 轮不写 |
| turnHeader | `historyRoundCount`     | 无（TS 内部用于 fork 边界）                                                              | 收口时写入本轮写入历史的模型轮次数（每次提交的模型响应算 1 次，compact 摘要算 1 次）                       |
| userInput  | `rootSourceCommandId`   | 分享投影                                                                                 | 等于 `sourceCommandId`：Rust 没有派生输入的 provenance                                                     |
| toolCall   | `assistantResponseId`   | `conversationCuaGroups`：按响应把 Computer Use 工具行分组                                | 取发出该工具调用的模型响应 id，与同一响应的 assistantText 行一致                                           |
| toolCall   | `input`                 | 结构化展示                                                                               | `inputText` 的 JSON 解析结果；解析失败时不写                                                               |

不处理（仅 id 取值方式不同，不影响语义）：

- Node 的 userInput `entityId` 取 turn id；
- Node 的 assistantText `entityId` 取 `assistantResponseId`。

## 所有者

- 行由 core `Engine` 的事件投影写入（`Session::row` 与 `event_projection.rs`）。
- 响应 id 由 model 层的 `TextBuffer` 在每次请求尝试时生成：
  - `ModelOutput.response_id` 取最终成功的那次尝试；
  - 经 `Event::ModelDone` 送达 Engine，存放在活跃 run 上；
  - 随后的 `ToolStart` 用它给工具行打标。

## 验收

- `zcode-cli-rust-differential.test.ts` 逐行比较字段集合与上述字段的值（id 与时间戳归一化）；
  出现新的差异字段即失败。
- goal 差分覆盖 `/goal` query 轮的 `executionKind`。

## `/goal` 的轮次结构（差分中发现，已对齐）

Node 把 `/goal` 拆成两轮，Rust 之前在同一个 agent 轮里执行：

|                | Node（oracle）                                                                                                                  | Rust 原行为                          | 现行为  |
| -------------- | ------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------ | ------- |
| query 轮       | `userInput` header（`controlOnly`，立即 `completedSuccess`，没有 `activeMs` 与 `historyRoundCount`）+ userInput 行（`canEdit`） | 同一轮 `agent`                       | 同 Node |
| 执行           | 随后的 `goalContinuation` 轮（`agent`）；回复只有 `canFork`，没有 `canRetry`                                                    | 在 query 轮内执行，回复有 `canRetry` | 同 Node |
| `goalSet` 标记 | 不产时间线行（stateOnly；隐形行会污染轮次分组）                                                                                 | 产生 `timelineMarker(goalSet)`       | 不产生  |

- 输入边界仍记在 query 轮上（edit/rewind 以它为准）。
- 本次输入的行从 query 轮 header 起发布（`new_input_rows`），两轮在同一批增量中下发。
- 验收：`zcode-cli-rust-title-differential.test.ts` 逐行比较 `/goal` 会话的轮次归属、种类、origin、executionKind、
  状态与 actions，两侧一致。
