# 生成速度由 Renderer 本地观测

- 状态：Accepted
- 日期：2026-10-12

## Context

会话视图需要在输入框工具条显示当前对话的实时 token 生成速度（见
[docs/specs/conversation-generation-speed.md](../specs/conversation-generation-speed.md)）。约束来自协议事实：

- 模型 provider 只在每次模型请求结束时给出 usage；流式阶段下发的只有正文、思考与工具参数的文本 delta，
  没有任何逐 chunk 的 token 计数（`apps/escode-cli/packages/core/src/runtime/methods/model.ts` 只在
  `finish` 分支写入 usage）。
- 因此"实时速度"的分子在任何一层都只能是按流出文本的估算，运行时并不比客户端更准。

考虑过两种所有权方案：

- **Renderer 本地观测**：复用 v4 conversation topic 帧流，按本地单调时钟做滑动窗口。观测器的先例是
  `packages/ui/src/v4/telemetry/localTtftObserver.ts`：Renderer 独占本地时钟，只采信 `online` 投递，
  `recovery` 与 snapshot resync 一律排除。
- **Runtime 计算后经协议下发**：新增 v4 state 字段与 delta，由 CLI runtime 计算速度。跨设备读数一致、
  重连后由服务端续算，但 runtime 同样拿不到逐 chunk token 数（要么自己分词，要么仍然估算），且需要新增
  schema、投影、双链路验证与 spec 预算，成本高一个量级。

## Decision

采用 Renderer 本地观测。`packages/ui/src/v4/telemetry/localGenerationSpeed.ts` 是本特性的唯一所有者：

```text
v4 topic 帧流（online delta）
        │
        ▼
LocalGenerationSpeedTracker ──▶ 按 sessionId 的观测状态（窗口 / 本轮累计 / 缺口）
        │                                   │
        │                                   └──▶ 保留值：模块级内存注册表（不随投影 store keep-warm 释放丢失）
        ▼
useLocalGenerationSpeed（useSyncExternalStore）──▶ 工具条读数组件
```

- 实时值为 3 秒滑动窗口：窗口内估算 token ÷ 窗口时长；窗口排空（工具执行、等待授权、等待用户回答、模型停顿）
  显示占位符，不显示 0。
- 本轮平均的分母只统计本地观测到的生成时间，排除工具执行与等待交互区间；出现过投递缺口的轮次不显示平均值。
- 分子在轮末用 `turnHeader.outputTokens`（服务端计量）修正，仅在无缺口时生效。
- 估算复用 `packages/shared` 的 `estimateTokens`，CLI 上下文预算与 UI 读数共用一份口径。

## Consequences

- 零协议改动：不新增 v4 schema、投影字段或运行时投影，Desktop `desktop-continuous` 与手机
  `web-remote-replayable` 复用同一观测器，差异只由投递类型表达。
- 读数天然是**本机视角**：手机断线重连后窗口重新累积，该轮不留平均值；桌面与手机在同一轮上的数值允许不同。
  这是刻意的取舍——速度是本地体感指标，不是会话事实。
- 保留值只在内存中，应用重启即清空；不做历史图表与持久化统计。
- 运行时不持有第二份速度事实，未来若要跨设备一致读数、历史统计或按请求精确 TPS，必须另立规格在 runtime
  侧新增投影，不能从客户端结果反推服务端事实。
