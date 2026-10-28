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
- 本机这一次运行 Rust 比 Node 热态慢约 20%、冷态慢约 36%；但 CI 同用例（run `40512267700`）
  五次采样中位数 Node 140.6 ms / Rust 147.2 ms（差 4.7%），说明该差距在运行间波动内。
  只有冷态首条稳定高于热态约 150–230 ms（Node 301–319、Rust 376–410），是需要 profile 的部分。

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

CI 实测（run `40512267700`，ubuntu-latest，5 条采样）：

| runtime  |   中位数 | 采样值                                |
| -------- | -------: | ------------------------------------- |
| Node CLI | 140.6 ms | 319.4 / 150.5 / 136.7 / 135.8 / 140.6 |
| Rust CLI | 147.2 ms | 375.7 / 175.8 / 131.6 / 145.1 / 147.2 |

## 4. 验证

- 本地 Node/Rust 各跑通一次（5 条采样），数据见上表。
- `pnpm --filter @escode/desktop typecheck:e2e`、`pnpm typecheck`、`pnpm lint`、
  `pnpm architecture:check --changed` 通过。
- CI：`desktop e2e matrix` 的 `app perf` 作业会产出 Node/Rust 首段 JSON 与 summary 表。

## 5. 未完成

- 首段优化（冷态 410 ms、热态 Rust 179 ms vs Node 149 ms）。
- 真实 provider 首字延迟（本用例 fixture 固定 1 ms）。

## 6. 分阶段探针（2026-10-28 补充）

`app-perf-first-segment.perf.ts` 增加 E2E 专用探针
（`window.__ESCODE_FIRST_SEGMENT_PROBE__`，仅 `VITE_ESCODE_E2E_STORE_BRIDGE=1` 构建注册）：
记录 `submit → 首个会话帧 → 首个投影行 → 首个 assistant 文本` 四个时间戳，产出
`phaseMs.submitToFrame / frameToRow / rowToText`。已加单元测试
`packages/ui/test/firstSegmentProbe.test.ts`。

本文件中的 2026-10-27 数字仍是无分阶段数据的基线；冷态各段占比需等下一次
`desktop e2e matrix` 的 app-perf 产物落库后再补。在拿到分段数据前不调整投递或渲染节奏。
