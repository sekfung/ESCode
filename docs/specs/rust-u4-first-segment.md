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

### 2.1 分阶段探针（2026-10-28）

为定位冷态首条的固定成本，E2E 构建额外注册一个仅测试可见的全局对象
`window.__ESCODE_FIRST_SEGMENT_PROBE__`（`VITE_ESCODE_E2E_STORE_BRIDGE=1` 时才注册；
生产构建不存在）。用例每轮发送前调用 `arm()`，随后记录：

| 阶段       | 打点位置                                           | 含义                    |
| ---------- | -------------------------------------------------- | ----------------------- |
| submit     | DOM `submit` 事件（用例内监听）                    | 用户提交                |
| firstFrame | `SessionDataLayer.transport.onFrame` 进入 store 前 | 首个会话帧到达 renderer |
| firstRow   | `store.handleFrame` 返回后                         | 投影行已并入本地窗口    |
| firstText  | MutationObserver 首次看到 assistant 文本增长       | React 渲染出首字        |

输出 `phaseMs.submitToFrame / frameToRow / rowToText`，冷态取第 1 条、热态取后续中位数。
探针缺失时字段为 null，用例仍按原有总时长断言通过。

## 3. 结果（Linux Xvfb，release 运行时，2026-10-27）

| runtime  |   中位数 | 5 条采样                              | 首条（冷） | 后续中位数 |
| -------- | -------: | ------------------------------------- | ---------: | ---------: |
| Node CLI | 149.3 ms | 301.0 / 143.2 / 149.3 / 136.9 / 167.1 |   301.0 ms |   149.3 ms |
| Rust CLI | 179.6 ms | 409.8 / 207.1 / 178.2 / 171.4 / 179.6 |   409.8 ms |   178.2 ms |

- 热态（第 2 条起）：Rust 171–207 ms，中位数约 179 ms，**在 180 ms 目标附近**；Node 137–167 ms。
- 冷态首条：Rust 409.8 ms、Node 301.0 ms，**均超过 180 ms**。这是 App/会话首次发送的固定成本，
  与模型延迟无关。
- 本机单次运行 Rust 热态比 Node 高约 20%；CI 五次采样（run `40512267700`）中位数 Node 140.6 ms /
  Rust 147.2 ms（差 4.7%），说明热态差距在运行间波动内。冷态首条两侧均稳定高于热态
  150–230 ms。

## 4. 结论与后续

- 旧口径的 1.38 s 作废，真实 App 侧首段是 180–410 ms 量级。
- §5 目标「≤180 ms」在热态 Rust 中位数上刚好达到边界，**冷态首条未达到**；判定为部分达成。
- Rust 相对 Node 的 20–36% 差距需要单独 profile（会话创建、Host 投递、row 投影三段的耗时分解），
  不在本切片内做优化，以免在未定位的情况下调整投递或渲染节奏。

## 5. 非目标

- 在分阶段数据落库前不调整投递/渲染节奏；2026-10-28 只补了分阶段探针，尚无优化结论。
- 不覆盖真实 provider 的首字延迟（fixture 固定 1 ms）；该项由真实 provider benchmark 单独记录。
