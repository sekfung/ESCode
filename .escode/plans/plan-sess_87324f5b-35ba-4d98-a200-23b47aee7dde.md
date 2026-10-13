# 生成速度按 dsh 口径改造：拆所有权 + 修正偏差

## 一、结论：两个量，两个所有者

现在的实现把两个不同的量塞进了同一个本地观测器，这是"不准确"的根源：

| 量                | 含义                     | 权威数据在哪           | 本次归属                       |
| ----------------- | ------------------------ | ---------------------- | ------------------------------ |
| 轮末解码速度      | 这一轮模型解码有多快     | provider usage + runtime 对流的观测 | **CLI 投影（host 侧、权威、可持久）** |
| 实时接收速率      | 我这边此刻收到多少 token | 只有本机能观测         | **Renderer（保持本地）**       |

理由：轮末值当前就是"服务端 token 数 ÷ 本地分母"的混合口径，两边都不权威；而它既然用 provider 计数，就该连分母一起用 runtime 的观测。实时值没有任何一层有真实逐 chunk token 数（ADR 0006 已论证），且它就是本机体感，留在 Renderer 更诚实。dsh 的架构（host 侧持久投影）对轮末值是对的——但它没有实时值，所以实时值不跟着搬。

顺带消掉三处复杂度：轮末值改为 host 权威后，本地"阻塞区间"启发式、缺口不显示规则、结算延迟逻辑都不再需要。

## 二、口径（dsh + 修正）

**轮末解码速度**（每次模型调用一个区间，按 token 加权聚合）：

```
rate = Σ(tokensᵢ − 1) / Σ(spanᵢ / 1000)
```

- `spanᵢ` = 该次调用的 **首个生成 delta → 最后一个生成 delta**（runtime 本地时钟）。相对 dsh 的修正①：dsh 用"消息装配完成"作终点，把收尾 chunk 与落库时间算进分母；runtime 直接观测末 token，能取到真终点。
- `tokensᵢ` = 该次调用 provider 上报的 `outputTokens`。修正⑤：区间起点取首个生成 delta（含思考/工具参数），与 provider 的 output 口径对齐。
- 修正③：无 token、`span ≤ 0`、`tokens < 2`、缺 usage 的调用，**分子分母一起剔除**，而不是把时间钳到 0 还留着 token。
- 修正②：重试开启新的调用区间，失败尝试整段不计（runtime 可见 `llm/retry` 边界）。
- 修正④：`tokensᵢ − 1`，因为 `span` 覆盖的是 N 个 token 之间的 N−1 个间隔。
- 修正⑥：跨设备一致、分页与压缩不影响，可回归。

**实时接收速率**（Renderer，语义不变）：3 秒滑窗内估算 token ÷ 窗口时长；保留工具执行/等待交互期间显示 `—` 的既有产品规则。仅补两处：探测到同一行文本回退（重试重开）时重置该行计数；起步归一沿用。

## 三、改动清单

**1. Spec 先行**（AGENTS.md 要求）
- 重写 [`docs/specs/conversation-generation-speed.md`](C:\Users\sekfung\Projects\ESCode\docs\specs\conversation-generation-speed.md)：定义上表两个量各自的所有者、口径、显示矩阵与验收；写明为何实时值不追求跨设备一致而轮末值必须一致。
- 新增 ADR `0007`：记录拆分所有权与理由，取代 [ADR 0006](C:\Users\sekfung\Projects\ESCode\docs\adr\0006-generation-speed-observed-in-renderer.md) 中"轮末值也由 Renderer 观测"的部分（0006 保留为历史，状态改为 Superseded by 0007）。

**2. 协议**
- [`rows.ts`](C:\Users\sekfung\Projects\ESCode\packages\shared\src\escode-protocol-v4\rows.ts) 的 `turnHeaderRowSchema` 增加 `decodeMs?`、`decodeTokens?`（与 `outputTokens` 同类，optional 兼容旧快照）。只下发原始和值，派生速率留在展示层。
- 同步 `packages/shared/src/escode-protocol/index.ts` 的导出与运行时校验。

**3. CLI runtime 采集**
- 在消费模型流处记录每次调用的首/末生成 delta 时间戳（`core/src/runtime/methods/model.ts` / `turn-model-step.ts`，该处已有 `modelStartedAt` 等时间基准）。
- 重试由 `adapters/src/model/runner-stream.ts` 的 attempt 循环界定，每次 attempt 一个区间。
- 新时序随 `ModelCompletePayload` 下发（`contracts/src/events/session.events.ts`），复用现有 `isStepUsageModelComplete` 的主链路判定。

**4. CLI 投影**
- [`product-projection.ts`](C:\Users\sekfung\Projects\ESCode\apps\escode-cli\packages\bootstrap\src\escode-protocol-v4\product-projection.ts) 在 `ModelComplete` 分支按现有 `outputTokens` 的方式累计 `decodeMs`/`decodeTokens` 到 turnHeader（约 4849 行处），重试清理沿用 `setApiRetry` 附近的既有模式。

**5. Renderer**
- [`localGenerationSpeed.ts`](C:\Users\sekfung\Projects\ESCode\packages\ui\src\v4\telemetry\localGenerationSpeed.ts) 收缩为只做实时窗口：删除 `activeMs`/`blocked`/`commitSettle` 的估算路径。
- 轮末平均值改读 `turnHeader.decodeMs`/`decodeTokens`；缺字段（旧 CLI）时显示未知，不用另一套口径顶替，避免同一标签下静默换含义。
- 保留值的弱化样式、下一轮清空、`data-generation-speed-*` 锚点与 aria 文案沿用。

**6. 测试与验证**
- CLI 投影单测：区间累计、重试、零宽/单 token 剔除、跨 step 加权。
- Renderer 单测：窗口语义（[`packages/ui/test/localGenerationSpeed.test.ts`](C:\Users\sekfung\Projects\ESCode\packages\ui\test\localGenerationSpeed.test.ts)），轮末值来源切换。
- E2E 更新 [`conversation-session-generation-speed.test.ts`](C:\Users\sekfung\Projects\ESCode\packages\desktop\test\e2e\conversation-session\manual-review\pending\conversation-session-generation-speed.test.ts) 与 `docs/test-cases/e2e/conversation-generation-speed.feature`。
- 执行 `pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed` 及相关包测试，如实报告结果。

## 四、兼容性

旧 CLI + 新 Desktop：无 `decodeMs` → 轮末显示未知（不回退估算口径）。
新 CLI + 旧 Desktop：新字段被忽略，行为同今。
实时值两端行为不变。

## 五、已知取舍（写进 spec）

- provider 的 `outputTokens` 是否含思考 token 取决于 provider；本口径下区间起点同样取首个生成 delta，故两者一致，但跨 provider 的数值不可直接横向比较。
- 轮末值不再反映本机链路状况，实时值才反映；这是刻意的分工，需在文案上可区分。
