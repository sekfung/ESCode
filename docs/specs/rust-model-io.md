# Rust model-IO 记录（模型调用轨迹）

2026-10-02。方法级 diff 把 `workspace/updateModelIoPreferences` 记成「App 容忍 method-not-found，只是设置不生效」。
核查后发现缺口更大：**Rust 根本不写 model-IO 记录**，所以 App 的「模型调用轨迹」侧栏
（`packages/services/src/escode-agent/modelTrajectory.ts::readModelTrajectory`，读
`~/.escode/cli/{debug,rollout}/model-io-<session>.jsonl`）对所有 Rust 会话都是空的。全量保留开关只是这个缺口露在外面的一角。

## TS 基准

- 写入：`apps/escode-cli/packages/adapters/src/model/runner-debug.ts`（`recordGenerateTextDebug` / `recordStreamTextDebug`
  → `writeModelIODebugRecord`），脱敏在 `runner-debug-redaction.ts`。每次 HTTP 尝试一条 `type: "model_io"` 记录。
- 开关：`ESCODE_RUNTIME_ENV=test` 不写；`development` 写 `~/.escode/cli/debug`；其余（含未设置）写 `~/.escode/cli/rollout`。
- 一个 session 一个 `model-io-<sanitized sessionId>.jsonl`（sessionId 只保留 `[A-Za-z0-9_-]`、其余折叠为 `-`、去首尾 `-`、
  截 80；缺失为 `no-session`），新请求 append。
- 生产（rollout）：新 session 文件前把目录内 `model-io-*.jsonl` 按 mtime 淘汰到最多 3 个；单文件 64 MiB 上限
  （debug 256 MiB），超限时整文件重写为当前记录并带 `modelIOReset`；删 `request.sdkMessages`、成功时删
  `request.body.messages`、删 `response.body`。
- 压缩：`request.messages` / `sdkMessages` / `body.messages` 相对同文件上一条做 delta（`*Kind: "delta"` +
  `*Offset`），无进程内状态或历史被改写时写 baseline（超过 64 条（debug 256）写 `tail`，否则 `full`）；
  失败记录的 `body.messages` 总是 `full`。
- 全量保留（`workspace/updateModelIoPreferences` → `fullRetentionEnabled`）：仍脱敏，但跳过淘汰、上限、生产裁剪与压缩。
  偏好对已有与之后的 session 都立即生效。

## App 实际消费的字段（验收以此为准）

`readModelTrajectory` 只读：`type`、`sessionId`、`requestId`、`attempt`、`startedAt`、`completedAt`、`durationMs`、
`turnId`、`traceId`、`querySource`、`model.{modelId,providerId,role,source}`、`request.messages`（role + content 文本/
parts，tool 消息的 `toolCallId`/`toolName`/`isError`）、`request.toolNames`、`response.{finishReason,text,reasoningText,
toolCalls[{id,name,input}],usage{inputTokens,outputTokens,totalTokens,cacheReadTokens,reasoningTokens},responseId,modelId}`、
`error.{name,message,stack}`，并按 `*Kind`/`*Offset` 把 delta 还原。headers、`sdkMessages`、`providerMetadata` 不进 UI。

## Rust 设计

- **写在 `HttpModel` 里**（model crate）：原始请求体、重试尝试都只在这里可见；与 TS 一样每次尝试一条记录，
  任何写入失败都不影响模型请求。
- **调用元数据**（sessionId / turnId / querySource / model role）由 core 在各调用点用 task-local 作用域提供
  （定义在 core-api，避免 `ModelPort` 签名变更扩散到所有实现）；缺失时 `sessionId` 为空（文件名 `no-session`）。
- `request.messages` 由 Rust 内部 OpenAI-chat 形历史投影成 TS `ModelInputMessage` 形：assistant `toolCalls`
  `[{id,name,input}]`、tool 消息 `toolCallId`/`toolName`/`isError` + 字符串 content；`_escode_*` 内部字段不落盘。
- **不写 headers**：UI 不读，且请求头里有鉴权；TS 写了但要过一层脱敏。少写比漏脱敏安全，差异记录在案。
- 压缩状态只在 Rust 进程内使用，指纹算法不必与 TS 字节一致（只决定 delta 还是 baseline），但 delta/baseline
  的输出语义必须让 App 还原出相同的消息序列。

## 分期

1. 记录器：目录/开关、单 session 文件、淘汰与上限、生产裁剪、delta/tail/full 压缩、记录字段（主回合、子代理、
   标题、compact 都经 `HttpModel`，各自带 querySource）。
2. `workspace/updateModelIoPreferences`：进程级偏好 + 全量保留模式；回显 `{workspace, fullRetentionEnabled,
updatedSessionCount}`。

## 验收

- App 差分：同一 fixture（`ESCODE_RUNTIME_ENV=development`，HOME 指临时 root）跑同一段多回合对话（含工具调用），
  分别用 App 的 `readModelTrajectory` 读 Node 与 Rust 的 model-io 文件，比对映射后的 `records`
  （去掉 requestId/时间/耗时/responseId 等非确定字段）：callSource、model、request.messages 序列、toolNames、
  response 文本/工具调用/usage/finishReason 一致。
- 单测：淘汰到 3 个文件、超限重置、delta → 超长 baseline 写 tail、失败记录 body 保持 full、`test` 环境不写。

## 实现与验证（第 1、2 期，2026-10-02）

- 记录器：`crates/model/src/model_io.rs`（`HttpModel::complete_inner` 每次尝试一条），调用元数据
  `crates/core-api/src/model_call.rs`（task-local `ModelCallScope` + `with_query_source`）。querySource 覆盖：
  主回合 `main_turn` / 子代理 `subagent`（`run.rs`）、`compact`、`session_title`、`project_memory_extract`、
  `target_completion_verification`、`web_fetch_processing`、`read_session_context`、`web_search_tool`、
  workspace generate-text（连通性测试 `provider_settings_connectivity`，否则调用方给的 querySource）。
- `workspace/updateModelIoPreferences`：`crates/core/src/app/model_io_preferences.rs`，进程级开关，
  `updatedSessionCount` 为当前内存中的会话数。
- tool 消息的 `toolName`：持久化历史只在带媒体时保留 `_escode_tool_name`，投影时按 `tool_call_id` 回查前面
  assistant 的 `tool_calls`。
- 性能：生产态成功记录要删 `body.messages`，此时用 `RawValue` 跳过它，不为可达近百 MB 的附件请求体构建值树。

与 TS 的差异（记录在案，App 可见内容一致）：

- **落盘压缩形态**：Node 的 canonical messages 带随最新消息移动的 cache 标记，前缀指纹对不上，主回合通常写
  `full`；Rust 投影后没有这些标记，能写合法的 `delta`。两者经 App `readModelTrajectory` 展开后逐条相同。
- 不写 `request.headers` / `sdkMessages` / `providerMetadata` / `response.body` / `response.modelId`（UI 不读；
  headers 含鉴权）；error 记录为 `{name: <failure code>, message}`，没有 JS stack。
- 被取消（`cancel` 抢占）的尝试不落盘：TS 会记一条 abort 错误。
- 标题请求：Node 非流式、Rust 流式（既有传输差异，不影响记录内容）。

验收：`packages/services/tests/escode-cli-rust-model-io.test.ts`——两回合（含推理、工具调用、工具结果、第二回合）

- 标题，Node 与 Rust 的 model-io 文件经 App `readModelTrajectory` 读出的 4 条记录逐条一致（callSource、model、
  request.messages、toolNames、response 文本/推理/工具调用/usage/finishReason）；全量保留模式下偏好回显一致、
  落盘不带压缩标记、轨迹一致。单测覆盖 delta / 历史改写回 baseline / tail、生产裁剪与失败保留、淘汰到 3 个文件、
  全量保留、消息投影、请求体跳过 messages。
