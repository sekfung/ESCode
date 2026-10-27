# Rust 阶段 4：首条消息第一段测量（rust-unified-host.md §5）

父计划：`docs/specs/rust-unified-host.md` §阶段 4 第 3 项（首段 ≤180 ms）。
相关：`docs/specs/rust-u4-startup-exit.md`（退出部分已交付）。

## 1. 问题

此前 App 级性能用例用 `waitForV4AssistantMessageContaining`（WebDriver 轮询，默认 500 ms）
给「点击 → 首段可见」计时，得到 1.3–1.5 s。该数字的粒度误差和 WebDriver 往返占比过大，
不能用来判断 §5 的 180 ms 目标。

## 2. 测量契约

- 用例：`packages/desktop/test/e2e/perf/app-perf-first-segment.perf.ts`（`.perf.ts`，不进默认套件）。
- 计时点：
  - 起点 = 渲染进程里真实 `submit` 事件派发时刻（`requestSubmit` 触发），不是 WebDriver 调用前；
  - 终点 = MutationObserver 首次观察到 assistant 行文本相对基线增长的时刻。
  - 两者都用同一渲染进程的 `performance.now()`，不含 WebDriver 往返。
- 负载：fixture `E2E_APP_PERF_FIRST_<n>` 首个文本 delta 在 1 ms，因此结果≈提交→IPC/引擎投影→
  首字渲染的 App 侧开销，**不含真实模型首字延迟**。
- 采样：同一会话连续 5 条消息；首条（冷）与后续（热）分别记录，中位数按 5 条计算。

## 3. 结果（Linux Xvfb，release 运行时，2026-10-27）

| runtime  |   中位数 | 5 条采样                              | 首条（冷） | 后续中位数 |
| -------- | -------: | ------------------------------------- | ---------: | ---------: |
| Node CLI | 149.3 ms | 301.0 / 143.2 / 149.3 / 136.9 / 167.1 |   301.0 ms |   149.3 ms |
| Rust CLI | 179.6 ms | 409.8 / 207.1 / 178.2 / 171.4 / 179.6 |   409.8 ms |   178.2 ms |

- 热态（第 2 条起）：Rust 171–207 ms，中位数约 179 ms，**在 180 ms 目标附近**；Node 137–167 ms。
- 冷态首条：Rust 409.8 ms、Node 301.0 ms，**均超过 180 ms**。这是 App/会话首次发送的固定成本，
  与模型延迟无关。
- Rust 比 Node：热态中位数高约 20%，冷态高约 36%。

## 4. 结论与后续

- 旧口径的 1.38 s 作废，真实 App 侧首段是 180–410 ms 量级。
- §5 目标「≤180 ms」在热态 Rust 中位数上刚好达到边界，**冷态首条未达到**；判定为部分达成。
- Rust 相对 Node 的 20–36% 差距需要单独 profile（会话创建、Host 投递、row 投影三段的耗时分解），
  不在本切片内做优化，以免在未定位的情况下调整投递或渲染节奏。

## 5. 非目标

- 不做首段优化；不改投递/渲染节奏。
- 不覆盖真实 provider 的首字延迟（fixture 固定 1 ms）；该项由真实 provider benchmark 单独记录。
