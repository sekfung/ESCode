# Rust 跨日 reminder

2026-09-26。对齐 TS `runtime/methods/turn.ts#injectDateChangeReminderIntoMessageHistory` 与
`helpers/runtime-reminders.ts#buildDateChangeReminderBody`。差分中发现 Rust 缺少该 reminder：
会话跨过本地午夜后，Node 在下一轮告诉模型新日期，Rust 仍沿用 system prompt 中的旧日期。

## 规则

- 状态：每个会话运行时在内存中记住上一轮的本地日期（`YYYY-MM-DD`，TS `formatLocalIsoDate`）。
  不持久化；冷加载（含 LRU 驱逐后重新激活）即重置，与 TS resume 把 `lastEmittedLocalDate` 置空一致。
- 每个新轮次开始时记录当前本地日期：首轮只记录；与上一轮不同时，在本轮用户输入之前追加一条
  hidden user 消息 `<system-reminder>\n{body}\n</system-reminder>`，`_zcode_source = "date_change"`；
  body 取自 TS builder 生成的资产（`prompt_templates.json#dateChange`）。
- 注入点：用户输入 admission（位于 `#sess_*` reminder 之后，与 TS 顺序一致）与 Goal 恢复开始的新 run。
- reminder 属于本轮输入边界之后的历史：编辑/重试截断时随本轮一起移除；不计为真实用户轮次。

## 所有者

- 本地日期由 `RuntimeClock::local_date`（Host `SystemClock` 用 `chrono::Local`）提供；测试时钟可不提供，
  不提供时不注入。
- 会话 owner 在 admission 时读取并写入会话消息，不经工具或模型 adapter。

## 边界

- Goal 验证未通过后的自动续跑在同一个 run 内把续跑消息直接交给运行中的循环；在那里插入 reminder 会让
  循环的工作上下文与 canonical 历史不一致，因此不注入。TS 的续跑是新的 executeTurn，会检查日期。
  差异只在两次迭代之间跨过本地午夜时出现，下一次用户输入或 Goal 恢复会补上。

## 验收

- `crates/domain/src/prompt.rs` 单测：首轮与同日不提醒；跨日文案与包装逐字符合 TS。
- `tests/date_change.rs`：进程内 Engine 连续四轮，首轮/同日无 reminder，改日期后恰好一条且位于用户输入之前，
  之后同日不重复；去掉 admission 注入时该用例失败（已做变异验证）。
- `scripts/generate-zcode-cli-rust-prompt.mjs --check` 覆盖文案漂移。
