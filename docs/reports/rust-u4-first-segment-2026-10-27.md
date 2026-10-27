# Rust 阶段 4：首段测量口径修正（2026-10-27）

父计划：`docs/specs/rust-unified-host.md` §阶段 4 第 3 项。契约：`docs/specs/rust-u4-first-segment.md`。

## 1. 结论

旧口径的「点击 → 首段可见 1.3–1.5 s」是 WebDriver 轮询（默认 500 ms）测出来的，不能用于
180 ms 目标判定。改为应用内计时后：

| runtime  |   中位数 | 首条（冷） |   后续热态 |
| -------- | -------: | ---------: | ---------: |
| Node CLI | 149.3 ms |   301.0 ms | 137–167 ms |
| Rust CLI | 179.6 ms |   409.8 ms | 171–207 ms |

- 热态 Rust 中位数约 179 ms，**刚好在 180 ms 目标线**；冷态首条 410 ms 未达标。
- Rust 比 Node 热态慢约 20%、冷态慢约 36%，需要单独 profile。

## 2. 变更

- 新增 `packages/desktop/test/e2e/perf/app-perf-first-segment.perf.ts`：
  - 起点取渲染进程真实 `submit` 事件（不是 WebDriver 调用前），终点取 MutationObserver 首次
    看到 assistant 文本增长；两端同一 `performance.now()`。
  - 同一会话连续 5 条；记录首条（冷）与后续（热）。
- fixture 增加 5 条 `E2E_APP_PERF_FIRST_<n>`（首字 1 ms），隔离 App 侧开销。
- `desktop-e2e-matrix` 的 `app-perf` 作业增加 Node/Rust 两次首段测量与 summary 表。

## 3. 为什么不是优化

Rust 与 Node 的热态差距只有 ~30 ms、冷态 ~110 ms，尚未分解到「会话创建 / Host 投递 / row 投影 /
渲染」四段。在未定位前调整投递节奏或渲染批处理，容易用牺牲吞吐或可回放语义换数字。下一步应先做
同样的应用内计时 + 各阶段时间戳（session created / first frame received / first row projected），
再决定优化点。

## 4. 验证

- 本地 Node/Rust 各跑通一次（5 条采样），数据见上表。
- `pnpm --filter @escode/desktop typecheck:e2e`、`pnpm typecheck`、`pnpm lint`、
  `pnpm architecture:check --changed` 通过。
- CI：`desktop e2e matrix` 的 `app perf` 作业会产出 Node/Rust 首段 JSON 与 summary 表。

## 5. 未完成

- 首段优化（冷态 410 ms、热态 Rust 179 ms vs Node 149 ms）。
- 真实 provider 首字延迟（本用例 fixture 固定 1 ms）。
- 冷启动 ≤1.8 s（安装包 2.40 s）与 skill/plugin 目录共享。
