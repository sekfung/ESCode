# Rust 生产代码 unwrap/expect 基线（2026-10-14）

> 用途：docs/specs/rust-unified-host.md 阶段 0「unwrap/expect 基线快照」与「实现质量对齐工作包」P0#4 的进度表。
> 目标：按 crate 逐个清零后，把 `unwrap_used` / `expect_used` 逐 crate 加入 `[workspace.lints.clippy]`（渐进 deny）。

## 计数方法

Node 脚本扫描 `crates/*/src` 与根 `src` 的 `.rs` 文件：排除 `*_tests.rs`；以花括号深度跳过 `#[cfg(test)]` 块；
统计 `.unwrap()` 与 `.expect(` 出现次数（近似生产计数，字符串内的匹配极少且不影响趋势）。

## 基线（2026-10-14，P0#4 清理后）

| crate        |  unwrap | expect |
| ------------ | ------: | -----: |
| `core`       |     157 |      0 |
| `domain`     |      83 |     27 |
| `tools`      |      55 |     18 |
| `model`      |       9 |      0 |
| `app-server` |       3 |      0 |
| `state`      |       3 |      0 |
| `host`       |       2 |      2 |
| `protocol`   |       1 |      0 |
| `core-api`   |       0 |      0 |
| `tui`        |       0 |      0 |
| `wiki`       |       0 |      0 |
| root `src`   |       0 |      0 |
| **合计**     | **313** | **47** |

## 本轮清理（P0#4）

- `model/src/registry.rs`：7 处 `RwLock` unwrap → `snapshot()` 助手（poison 容忍：快照是整块替换的
  自包含值，中毒后仍是最后一次成功写入的完整状态）。
- `tools/src/mcp_hub.rs`：15 处 `RwLock` unwrap → `state()` / `state_mut()` 助手（同一策略）。
- 两个文件当前锁 unwrap 为 0（`grep` 校验），行数仍在 400 行上限内。

## 下一步清零顺序（按密度）

1. `core`（157）：热点文件来自质量对比报告——`subagents.rs`（18）、`questions.rs`（13）、`commands.rs`（13）、
   `queue_control.rs`（11）、`history_commands.rs`（9）。模型入参 JSON 直取改 `ok_or` + context。
2. `domain`（83 + 27）：`custom_command_template.rs`（9）、`bash_policy.rs`（8）、`option_map.rs`（expect 5）、
   `web_fetch.rs`（expect 4）。
3. `tools`（55 + 18）：`browser_broker.rs`（9）等；锁类已在本轮清零。

清零完成后按 crate 在 `apps/escode-cli-rust/Cargo.toml` 的 crate 级 `[lints]` 覆盖或直接加入工作区清单，
并同步更新本表。
