# Rust 订阅续传（WP6：远程/手机恢复）

2026-09-24。`zcode-cli-rust-resume-differential.test.ts` 实测：客户端带 `base {logEpoch, seq}` 重新订阅时，

| runtime | ACK `mode` | 首帧                                                                   |
| ------- | ---------- | ---------------------------------------------------------------------- |
| Node    | `resume`   | 一个 `deltas` 帧，`fromSeq = base.seq`，`toSeq` = 当前，含其后全部增量 |
| Rust    | `snapshot` | 整份 snapshot（Rust 不保留已发布增量）                                 |

snapshot 在协议上合法（server 可判定 base 无效），状态仍正确；但手机断线重连每次都整份重传，`web-remote-replayable` 的恢复语义名不副实。`desktop-continuous` 同样受影响。

## 规则

- 所有者：会话 owner。每个会话在内存中保留已发布增量的有界日志（`Session.delta_log`，不持久化），每批记录 `(fromSeq, deltas)`；按序列化字节数限额（每会话 1 MiB），超出时丢弃最旧批次。
- subscribe 带 `base`：`base.logEpoch == session.epoch` 且 `base.seq` 落在日志覆盖区间 `[最旧批 fromSeq, session.seq]` 内 → ACK `resume`，发送一个 `deltas` 帧（`deliveryKind: "initial"`，`fromSeq = base.seq`，`toSeq = session.seq`），内容为 `base.seq` 之后的全部增量（批内按下标切分，seq 与增量一一对应）。
- 其余情况（epoch 不符、base 早于保留窗口、base 超前、冷启动后日志为空而 base 落后）→ 维持 snapshot，不伪造连续水位。
- `base.seq == session.seq`：`resume` + 空 `deltas` 帧，与 Node 行为一致（待差分确认）。
- 不改变 sessions-index / workspace-config 主题（仍为 snapshot）。

## 验收

- 差分：两种 clientMode 下 ACK mode、首帧种类、`fromSeq`、是否到达最新、增量 op 集合与 Node 一致。
- Rust 单测：窗口外 base、epoch 不符回落 snapshot；批内切分正确。

## 实现与验证（2026-09-24）

- `crates/domain/src/delta_log.rs`（纯逻辑 + 单测：批内切分、窗口外/超前/断档回落、超预算丢弃最旧批）；`subscriptions.rs::publish` 记录每批，`subscribe` 命中窗口时 ACK `resume`。
- 差分：两种 clientMode 的续传（ACK、首帧种类、`fromSeq`、到达最新、恰好覆盖 `(base, current]`、变更种类）与 Node 一致；过期 `logEpoch` 两侧都回落 snapshot。
- 已知差异（不属于续传语义）：流式发布粒度不同——Rust 以 `row.delta` 推送文本分片，Node 以 `row.upserted` 覆盖；续传内容因此逐条不同；但正确性已按 App 自己的归约器验证：base 快照 + 续传增量经 `@zcode/shared` 的 `applyConversationDeltas` 归约后，与同一时刻的全新快照（除 `seq` 外）完全相等，Node 与 Rust 用同一规则均通过。
- 未覆盖：`v4/conversation/resync`（同订阅恢复）与流控 `drained` 后的补发仍用 snapshot；Rust 的 resync 入参形状与 `conversationResyncParamsSchema` 不同，需单独对齐。
