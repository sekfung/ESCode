# 生成速度拆分为两个量、两个所有者

- 状态：Accepted
- 日期：2026-10-13
- 取代：[0006 生成速度由 Renderer 本地观测](0006-generation-speed-observed-in-renderer.md) 中"轮末平均值也由 Renderer 观测"的部分

## Context

[0006](0006-generation-speed-observed-in-renderer.md) 把实时值与轮末平均值都交给 Renderer 的
`LocalGenerationSpeedTracker`：实时值用 `estimateTokens` 估算，轮末平均值用 `turnHeader.outputTokens`
作分子、本地实测生成时间作分母，并以本地"阻塞区间"启发式（工具执行、等待交互）排除停顿。

实现落地后暴露两个问题：

- **轮末值是混合口径**。它既然采信服务端的 output 计数，就说明它想回答的是"这一轮模型解码多快"——这是会话事实。
  但分母仍是本机观测：包含本机链路抖动、用启发式猜测哪些区间不算生成、并在投递缺口时整轮放弃读数。
  分子权威、分母不权威，两边都不成立的组合正是"读数不准确"的根源。
- **同一标签下两种含义**。实时值是"我这边收到多快"（本机体感），轮末值是"模型解码多快"（会话事实）。
  两者用同一个 `tok/s` 展示，却各自带着不同的偏差方向。

对照 [deepseek-harness 的 `sessionStats`](https://github.com/deepseek-ai/deepseek-harness)（`packages/session/session-stats/src/projection.ts`）：
它把解码速度做成 host 侧持久投影，按 step 聚合 `decodeTokens / decodeMs`。架构方向是对的——权威输入本来就
在 runtime 手里（provider usage 与它自己对流的观测）——但它的口径有三处系统性偏差：

1. 区间终点取"消息装配完成"，把 provider 收尾 chunk 与落库时间算进分母（系统性低估，输出越短越严重）；
2. 步内 `llm/retry` 保留首次尝试的首 token 时间，失败尝试与退避整段算进分母，而分子只有成功那次 usage；
3. 零宽区间被 `Math.max(0, …)` 钳到 0 后仍保留全部 token，单步即可注水整轮读数。

它没有实时值，所以实时值不在这条对照里。

## Decision

按量的性质拆所有权，并采用修正后的 dsh 口径。

**轮末解码速度：CLI 投影所有（会话事实）。**
新增 `turnHeader.decodeMs` / `decodeTokens`，与既有 `outputTokens` 同源同路径：runtime 在消费模型流时记录
每次调用的首个与末个非空生成 delta 时间，随 `ModelComplete` 下发，由 `product-projection` 按主链路口径累计。

```text
轮末解码速度 = Σ(tokensᵢ − 1) / (Σ spanᵢ / 1000)
spanᵢ = 末个非空生成 delta − 首个非空生成 delta
```

相对 dsh 的三处修正，加上一处口径修正：

- 区间终点改取**末个生成 delta**，不用消息装配完成——runtime 直接观测流，能取到真终点；
- 无生成 delta、`span ≤ 0`、`tokens < 2`、缺 usage 的调用**分子分母一起剔除**，不钳时间留 token；
- 重试由运行时边界天然处理：adapter 的 attempt 重试在构造上只能发生在任何可见生成 delta 之前，
  失败尝试不发 `ModelComplete`；core 级断流恢复是新的调用即新的区间。不需要额外的重试识别；
- 分子取 `tokens − 1`，因为 `span` 覆盖的是 N 个 token 之间的 N−1 个间隔。

**实时接收速率：Renderer 本地观测（本机体感）。**
维持 0006 的做法：3 秒滑窗内 `estimateTokens` 估算量 ÷ 窗口时长，工具执行与等待交互期间显示 `—`。
不搬去 host 侧：没有任何一层有真实逐 chunk token 计数（0006 已论证），搬过去仍要估算，却要新增协议字段、
高频状态流量与节流延迟；而"我这边收到多快"本来就是客户端才能回答的问题。

所有权拆分后，Renderer 侧不再需要本地结算：本轮平均值直接读 `turnHeader` 的权威字段，
`activeMs` 阻塞区间启发式、缺口轮次不留平均值、结算延迟三处逻辑随之删除。

## Consequences

- 轮末读数跨设备一致：Desktop `desktop-continuous` 与手机 `web-remote-replayable` 在同一轮上得到相同数值，
  不再允许因链路不同而分叉。它不随分页、压缩、重连变化。
- 轮末读数不再反映本机链路状况，实时读数才反映。这是刻意的分工，两者在文案上可区分
  （`实时生成速度` / `上一轮解码速度`）。
- 冷恢复后不保留：重启后最后的 turnHeader 由 transcript 重新合成，不含新字段，读数回到 `—`。
  本特性不做持久化统计（与 0006 一致）。若将来需要跨重启保留，参照 Highspeed 的
  `highspeed-metrics-persistence` 把终态写进 transcript。
- 旧 CLI + 新 Desktop：无新字段，轮末显示 `—`，不回退到本机估算——同一标签下静默切换含义比缺少读数更糟。
- 协议字段是 optional 累加，不改既有 `outputTokens` 语义；`conversationSharePublicProjection` 的
  allow-list 重建需同步，否则分享链路会静默丢掉新字段。
