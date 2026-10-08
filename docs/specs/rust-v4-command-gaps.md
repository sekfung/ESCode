# Rust V4 命令缺口（2026-10-03 复查）

把共享协议 `commandPayloadSchemas`（`packages/shared/src/zcode-protocol-v4/command.ts`）的全部命令类型与 Rust
`crates/core` 的处理逐条比对，App 在用、Rust 完全没有处理的命令共 5 个——Rust 对未知命令一律
`rejected / guard.capabilityUnsupported`，UI 动作直接失败。

| 命令                         | App 入口                                                 | 状态                                 |
| ---------------------------- | -------------------------------------------------------- | ------------------------------------ |
| `setAssistantFeedback`       | 助手回复点赞 / 点踩（`v4/SessionPane.tsx`）              | **已实现**，差分一致                 |
| `createSelectionSideSession` | 框选「问一问」副屏（`v4/SessionPane.tsx`）               | **已实现**，差分一致（已知差异见下） |
| `startSavedWorkflow`         | 设置页「已保存工作流」启动（`useSavedWorkflowLauncher`） | **已实现**，差分一致                 |
| `resumeWorkflowRun`          | 工作流运行侧栏「恢复」（`WorkflowRunSidePane`）          | **已实现**，差分一致                 |
| `amendWorkflowRunSettings`   | 工作流运行设置弹层（`useWorkflowRunPaneSettings`）       | **已实现**，差分一致（已知差异见下） |

Workspace hook 的四个命令（`requestWorkspaceHookReview` 等）经 `kind.contains("WorkspaceHook")` 整体转发给工具层，已覆盖。

另有一个「命令存在、语义缺失」的缺口：`cancelBackgroundWork {workId: dwfrun-…}`（run 卡 / 详情页的「取消」，三个入口
同一实现）——Rust 只认子代理与后台 Bash，对工作流 run 回 `noop`，取消按钮无效。**已修**，见下节。

## 工作流 run 的取消与恢复

TS：`runtime.cancelBackgroundTask`（initiator = user，非 strict）与 `app.resumeWorkflowRun`（`port.resume` + 追踪重臂）。
Rust 的 run 由工作流宿主（`__zcode-workflow-host`）执行，新增宿主方法 `run.cancel` / `run.resume`
（`apps/zcode-cli/packages/cli/src/workflow-host-runs.ts`），Engine 侧 `workflow_run_commands.rs`。

```mermaid
sequenceDiagram
  participant UI
  participant R as Engine 请求分派
  participant T as 后台任务
  participant H as 工作流宿主
  UI->>R: v4/command resumeWorkflowRun / cancelBackgroundWork(dwfrun-…)
  R->>R: 校验信封、冷激活会话，登记辅助请求（不占 actor）
  R->>T: spawn
  T->>H: run.resume / run.cancel
  H-->>R: 可能反向请求 actor.create / workflowRuns.prior（actor 空闲，可处理）
  H-->>T: {ok, runId} | {ok:false, reason, message?}
  T->>R: Event::AuxiliaryDone(ack)
  R-->>UI: ACK
```

- 宿主处理中会反向请求会话 owner，Engine 不能在 actor 里同步等应答，故走 `mcp/list` 同款辅助请求。
- 恢复拒绝：`failed / fault.command.workflowRunResumeRejected.<reason>`，`message` 为宿主诊断或
  `workflow run resume rejected: <reason>`；取消拒绝（未找到 / 已终态）：`failed /
  fault.command.backgroundWorkCancelRejected.<not_found|not_running>`，`message` 为
  `background work <id> was not cancelled: <reason>`——均与 Node 原文一致。
- 验收：`zcode-cli-rust-workflow-run-commands.test.ts`（actor 挂住时 UI 取消 → 停止通知 → UI 恢复 → actor 重跑结算 →
  完成通知；未知 run 的恢复 / 取消、已完结 run 的取消，两侧 ACK 与通知逐字一致）。

## setAssistantFeedback

TS 以 transcript metadata 为持久权威、事件推进投影。Rust 的会话行本身随会话落库，反馈直接写在 assistantText 行的
`feedback` 字段（`null` 删除字段），推送 `row.upserted`，与命令 ACK 同一次提交，冷恢复随行还原。

- CAS 命令：缺 `baseRevision` 报错；stale 在命令入口判定。
- 目标行不存在 → `stale / proto.staleTarget`；不是 assistantText → `rejected / guard.actionUnavailable`；同值反馈仍 `accepted`。
- 验收：`zcode-cli-rust-assistant-feedback.test.ts`（like / 重复 like / dislike / 清除 / userInput 行 / 不存在行 / 重启恢复，两侧逐字一致）。

## createSelectionSideSession

TS：handler + core `createSelectionSideConversation`。Rust `selection_side_session.rs`：

```mermaid
sequenceDiagram
  participant UI as SessionPane
  participant E as Engine::command
  participant S as Store
  UI->>E: createSelectionSideSession(parent, firstInput?)
  E->>E: 父会话 subagent_child → rejected guard.subagentReadOnly
  E->>E: 新建 child：继承父消息（运行中截到当前轮用户输入为止）+ 副屏边界提醒；不复制行/Goal/队列/后台
  E->>E: firstInput → 在 child 上 admit（父队列不参与）；首轮前落 model-initial marker
  E->>S: child + 父命令 ACK 同一事务提交
  E-->>UI: ACK {type:createSelectionSideSession, sessionId, input?}
```

- child：`sess_` 前缀 id、`taskType = selection_side_chat`、标题 `Selection side chat`（generated，不生成标题）、
  `listed = false`（不进任务列表）、继承父模式 / plan / followup、附件、Skill 目录与提示快照；模型取 firstInput
  的 `modelSelection`，缺省继承父会话。
- 副屏内 `sendGoalCommand / pauseGoal / resumeGoal / editUserQuery / retryTurn / forkAssistant / discardSharedContext`
  → `failed / guard.selectionSideChatRestrictedCommand`。
- 验收：`zcode-cli-rust-selection-side-session.test.ts`（ACK、子会话行、模型请求会话部分、sessionKind/标题、列表不可见、
  受限命令、无首条输入的副屏，两侧逐字一致）。

### 已知差异

- marker 的 `toThought`：Node 副屏 child 投影的 `config.thought` 为空串，Rust 填实际档位。
- ~~shell 环境变更提醒~~：2026-10-08 已对齐（副屏、fork、跨进程冷恢复同一实现），见 `rust-shell-resume-notice.md`。

## forkAssistant 的 child 身份与边界（2026-10-03）

- **已改**：child 原先沿用父会话身份（`task_type = interactive`、id 无 `sess_` 前缀、标题与父会话相同），
  也没有 fork 边界产物。现按 TS `buildForkedSessionInput` + `buildAtomicForkNotice` 对齐：
  `sess_<uuid>` / `taskType = fork` / `Fork of {parent.title}` / `titleSource = generated`，
  并追加 hidden model-only 提醒（`_zcode_source = "fork_notice"`）与被选轮末尾可见的
  `timelineMarker`（`lane = turnTailBoundary`、`marker = {type:"forkNotice",…}`）。
  逐字比较见 `zcode-cli-rust-fork-child.test.ts`。
- **验收已确认**：fork ACK、`session/list` 可见性与 `parentSessionId`、child 行、child 首次模型请求的
  会话部分（含 fork 提醒）与 Node 一致；`turnId` / `createdAtSeq` 两侧取值不同（Node 用新 turnId 且按父
  会话 seq 续号，Rust 复用被选行 turnId、child 自身从 1 起），差分测试对这两项归一。

### 已知差异

- ~~fork child 的 `session/read.session.mode`~~（2026-10-08 已修复，两侧都改）：
  - 规则（TS `session-fork.ts` 的 `commitAtomicConversationFork`）：child 的协作模式与 Plan 开关取**被选轮** assistant
    消息上的 execution state；父会话之后切换模式不影响 child。副屏 child 仍取父会话当前状态。
  - Node 缺陷：`buildForkedSessionInput` 把父会话**创建时**的 `permission` 写进 child 行，而 child 实际执行模式来自
    同一 bundle 的 execution-state entry，`session.mode`（显示 build）与 `settings.permission.mode`、实际执行（yolo）
    互相矛盾。根因有两处，都修：
    1. `buildForkedSessionInput` 照抄父会话创建时的 `permission`：child 行的 `permission.mode` 改取同一个
       execution state（`session-fork.ts`）；
    2. 冷恢复（`resume.ts`）按 execution-state entry 恢复模式时只写 config、不进事件流，而 `session.mode` 来自事件
       投影（无模式事件时回落默认 build）。恢复后若投影与权威执行状态不一致，补一条 `SessionModeChanged`
       （`source: "system"`），与其他改模式路径（`applyRuntimeExecutionState`）同样经事件流生效。
  - Rust 偏差：child 继承父会话**当前**模式。修复：回复边界 `history::State` 记录当时的 `mode` / `planEnabled`
    （旧边界缺席时回落父会话当前值），fork 用被选边界上的值（`history_commands.rs`）。
  - 验收：`zcode-cli-rust-fork-child-mode.test.ts`（第一轮 yolo、第二轮 build 后 fork 第一轮：两侧 child 的
    `session.mode` 与 `settings.permission.mode` 都是 yolo，父会话仍是 build）。
- ~~shell 环境变更提醒~~：2026-10-08 已对齐，fork child 首个请求的提醒与位置两侧逐字一致，见 `rust-shell-resume-notice.md`。

## session/read 的 session 投影（2026-10-03）

对齐 `session-mapper.ts` 的 `mapSessionInfo`，改了四处：

- 补 `traceId`：进程内会话与 `session/list` 一样投影 `s.trace_id`（TS `session.traceID ?? app.traceId`）。
- 补 `target`：无 Goal 时显式 `null`（TS `mapSessionGoal` 对无目标返回 null，不是缺字段）。
- `session.model` 只发 `{providerId, modelId}`：TS 从 `optionalModelSelectionFromString(getModel())` 解析，
  该解析只按 `/` 切分，不含 options；档位仍由 `settings.thoughtLevel.current` 与 `settings.model.current` 表达。
- 未命名的会话不发 `titleSource`：TS 直发会话记录里的该字段，新建会话时它还没设置（整个键消失）；
  Rust 的内部初值 `default`（`session_new.rs`）表示同一状态，读口用 `Session::titled` 还原成「不发送」。
  已有来源时两侧都发原值（首条输入后是 `first_input`，与 TS 的 stored 身份一致）。

验收：`zcode-cli-rust-session-read-session.test.ts`（新会话与跑完一轮后的进程内 `session/read`，两侧
`session` / `settings` 投影逐字段一致；用例同时自检 Node 侧确实带 `traceId`、显式 `target: null`、
`model` 只有两个键、新会话无 `titleSource`）。

**已补**：有 Goal 的会话现在也投影 `session.target`（TS `mapSessionGoal` + `zcodeSessionGoalSchema` strict）。
给 `Goal` 补了 `createdAt` / `updatedAt`（旧数据缺省 0，投影时退回会话时间），并把内部状态归一到协议词表：

| 内部状态                            | 协议状态        |
| ----------------------------------- | --------------- |
| `active` / `verifying` / `notSatisfied` | `active`     |
| `verified`                          | `complete`      |
| `paused`（预算耗尽 `exhausted()`）  | `budget_limited` |
| `paused` / `failed`                 | `paused`        |

验收：`zcode-cli-rust-goal-target.test.ts`（设 Goal 后 `session/read` 的 `session.target` 逐字段一致；
两侧同时自检 `createdAt` / `updatedAt` 是 number、`status` 在协议词表内）。

## startSavedWorkflow（2026-10-03）


设置页「已保存工作流」的「运行」原先在 Rust 上直接失败（未知命令 → `rejected / guard.capabilityUnsupported`），
现在按 Node `app.startSavedWorkflow` 的语义实现。解析 / 实参校验 / 编译 / 落工作副本 / 提交 run 仍由工作流宿主
用 TS 的同一段共享实现（`launchSavedWorkflowRun`）完成；Rust 只做会话侧事实。

| 事实                                                                    | 所有者           |
| ----------------------------------------------------------------------- | ---------------- |
| 已保存工作流的解析 / 实参校验 / 编译 / 工作副本 / `port.submit`          | 工作流宿主（TS） |
| run 的后台追踪（注册表、终态 waiter、结算通知）                          | 工作流宿主（TS） |
| 启动轮的标题 / 行（userInput、turnHeader）/ runtime history / ACK / 持久化 | Rust Engine      |

宿主新增两个方法（`apps/zcode-cli/packages/cli/src/workflow-host-runs.ts`）：

- `run.startSaved {session, cwd, name, scope?, args?}` → `{ok:true, runId, toolCallId, launchInputId,
  launchText, meta, titleInput}` 或 `{ok:false, reason, message?}`。**零会话副作用**：解析 / 校验 / 编译
  失败时没有 run、没有消息、没有行；成功即已 `port.submit`，并把合成 CreateWorkflow 描述子按 toolCallId 暂存。
- `run.track {session, toolCallId}` → `{ok:true}`：为 `run.startSaved` 提交的 run 重臂后台追踪（与
  `run.resume` 的重臂同一条 `BackgroundTaskTracker` 路径）。

```mermaid
sequenceDiagram
  participant UI
  participant R as Rust Engine
  participant H as 工作流宿主
  UI->>R: v4/command startSavedWorkflow
  R->>R: session_busy 判定（有活动 / 排队轮即拒，无副作用）
  R->>H: run.startSaved（辅助请求，不占 actor）
  H->>H: 解析 → 实参校验 → 编译 → 工作副本 → port.submit
  H-->>R: {ok, runId, toolCallId, launchInputId, launchText, meta, titleInput}
  R->>R: 落启动轮（标题 / userInput / controlOnly turnHeader / history）——唯一的会话写入
  R->>H: run.track（重臂追踪）
  R-->>UI: ACK accepted {type:"startSavedWorkflow", runId, toolCallId}
  H--)R: runSettled（run 终态）→ 既有后台结果轮
```

追踪必须在**启动轮落定之后**才重臂（Node 是先 `await emitControlOnlyUserTurn` 再 `trackBackgroundTask`）：
否则一个很快结算的 run 的完成通知会先落成后台结果轮，启动轮就跑到它后面去。Rust 因此把「落启动轮」
拆成 actor 内的一个事件（`Event::WorkflowLaunchTurn`），辅助任务等它回 ACK 后才发 `run.track`。

- 启动轮：标题取工作流名（`titleFromInput` + `title_source = "first_input"`，会话为空标题时）；
  userInput 行 `origin:"workflowLaunch"`、文本为启动句、`sourceCommandId` / `rootSourceCommandId` 都是
  `launchInputId`；turnHeader `origin:"workflowLaunch"`、`executionKind:"controlOnly"`、
  `state:"completedSuccess"`、`sourceCommandId = launchInputId`；两行带同一份 `workflowLaunch` 元数据；
  文本进 runtime history（role=user），模型下一回合以真实 user prompt 读它。
- 拒绝（宿主 reason）：`failed / fault.command.savedWorkflowStartRejected.<invalid_name|not_found|invalid_args|
  compile_failed|session_busy|start_failed>`，`message` 用宿主诊断，缺席时 `saved workflow start rejected: <reason>`。
  `session_busy` 由 Rust 在调宿主前判定（TS `hasActiveOrQueuedTurnWork()`），不产生任何副作用。
- 提交之后的失败（落行 / 重臂）只记 warn 不回滚：run 已在飞，可在侧板取消；ACK 仍按成功回。
- 验收：`zcode-cli-rust-start-saved-workflow.test.ts`（成功启动的 ACK、启动轮两行、`session/read` 标题与状态、
  下一次模型请求里的启动句、`invalid_name` / `not_found` / `invalid_args` / `session_busy` 四条拒绝的
  ACK 与零行副作用，两侧逐字一致）。

## amendWorkflowRunSettings（2026-10-04）

run 卡 / 详情页「配置」弹层的「应用」原先在 Rust 上直接失败（未知命令 → `rejected /
guard.capabilityUnsupported`），现在按 Node `app.amendWorkflowRunSettings` 的语义实现。判定与执行仍由
工作流宿主用 TS 的同一段实现完成——为此把 Node runtime 里的算法抽成 core 的纯函数
`applyWorkflowRunSettings`（`dynamic-workflow-run-settings-apply.ts`），Node runtime 与宿主各注入一份
依赖：前者拿 `AgentRuntimeInternal` 的会话事实与执行器，后者拿宿主的 cwd、run 端口与 Rust 递来的模型目录。

| 事实                                                                                                          | 所有者           |
| ------------------------------------------------------------------------------------------------------------- | ---------------- |
| 归属校验 / 可配置校验 / 两项三态归一 / 未改判定 / 脚本读取 / 编译 / 调并发 / `port.amend` / 工作副本 / 提交 run | 工作流宿主（TS） |
| 模型目录（provider 注册表 → `ModelCatalogEntry[]`，补 `current`）                                              | Rust Engine      |
| 设置轮的会话写入（userInput、controlOnly turnHeader、runtime history）                                          | Rust Engine      |
| run 的后台追踪重臂（登记表、终态 waiter、结算通知）                                                            | 工作流宿主（TS） |

宿主新增一个方法（`apps/zcode-cli/packages/cli/src/workflow-host-runs.ts`）：

- `run.amendSettings {session, cwd, runId, subagentModel?, maxConcurrency?, models?}` →
  `{ok:true, runId, toolCallId, supersededRunId?, track, turn:{text, meta, titleInput}}` 或
  `{ok:false, reason, message?}`。**零会话副作用**：失败时旧 run 照旧在跑，没有新 run、没有行、没有消息。
  成功即已提交新 run（或就地改完并发），并把合成 AmendWorkflow 描述子按 toolCallId 暂存。
  `track` 为假表示**就地调并发**（同一个 runId、没有后继），这条路上刻意不登记第二个后台任务。
  两项设置原样三态透传：键在场即用户改过，`null` 是「回到默认」。

```mermaid
sequenceDiagram
  participant UI
  participant R as Rust Engine
  participant H as 工作流宿主
  UI->>R: v4/command amendWorkflowRunSettings {workId, subagentModel?, maxConcurrency?}
  R->>H: run.amendSettings（辅助请求，不占 actor；附模型目录）
  H->>H: 归属 / 可配置 / 三态归一 / 未改判定 →（只改并发且 run 在飞）就地调并发 → 脚本 → 编译 → port.amend
  H-->>R: {ok, runId, toolCallId, supersededRunId?, track, turn}
  R->>R: 落设置轮（标题 / userInput / controlOnly turnHeader / history）——唯一的会话写入
  R->>H: run.track（仅 track=true；重臂追踪）
  R-->>UI: ACK accepted {type, runId, toolCallId, supersededRunId?}
  H--)R: runSettled（run 终态）→ 既有后台结果轮
```

- 拒绝（宿主 reason）：`failed / fault.command.workflowRunSettingsRejected.<not_found|not_configurable|
  unchanged|script_missing|model_unavailable|compile_failed|missing_boundaries|start_failed>`，`message` 用
  宿主诊断（编译 / 模型解析 / 启动失败原因），缺席时 `workflow run settings rejected: <reason>`。
- 模型目录：Rust 把 `registry.model_catalog()` 随请求递过去，并给会话当前 `providerId/modelId` 那一条补
  `current: true`（其余 `false`）。TS 端口的 `current` 是必需字段，它决定 `model_unavailable` 诊断里
  `[current]` 标记与「同名挂多个 provider」的第 3 档判定；缺了它两侧的失败文案会分叉。
- 设置轮的文本与元数据由宿主用 TS 同一个 `buildSettingsMessageText` / `boundWorkflowLaunchMeta` 生成
  （英文、进 provider transcript），Rust 只负责落行。

### 设置轮的时序

Node 把设置轮排进运行时命令队列（priority `next`），与通知同优先级、先于新 run 的任何通知。Rust 没有这条
队列，改用会话上的一个**待落列表**（`Session::settings_turns`，只存内存、不落库、`recover()` 清空）：
命令的 ACK 到达时先入列，会话空闲（无在跑轮、无待提升输入）即落行。

- 落行点固定三处：ACK 到达后、任意一轮 `Finished` 之后（**早于** `promote`）、以及上述两处的幂等重试。
  判据是 `!s.running() && s.queued_now.is_none()`。
- `deliver_workflow_notices` 增加 `!s.settings_turns.is_empty()` 门：只要还有设置轮没落，run 的完成通知就
  不能抢先变成本回合的后台结果轮。这是「追踪必须晚于设置轮重臂」（`run.track` 在落行之后才发）之外的第二道
  保险——即使宿主在 ACK 之前就报了结算，通知也排在设置轮后面。
- 设置轮不走模型：turnHeader 的 `executionKind = "controlOnly"`、`state = "completedSuccess"`、
  `origin = "workflowLaunch"`，与启动轮同形；`workflowLaunch` 元数据里的 `amend` 块区分两条路
  （有 `predecessorRunId` = 修订出新 run，没有 = 就地调并发）。

### 已知差异

- **设置轮与排队输入的先后**：Node 的运行时命令队列按到达顺序（同为 priority `next`）；Rust 一律先落
  已排队的设置轮、再提升排队输入。差别只在「命令行在处理中、用户既改了设置又排了输入」时可见。
- 拒绝 `start_failed` 的 `message`：宿主方法不存在时 Node 给 `dynamic workflow amend unavailable`，
  Rust 走宿主错误通道给 `fault.command.executionFailed`。宿主始终带该方法，属未接线的兜底面。

验收：`zcode-cli-rust-amend-workflow-settings.test.ts`（修订出新 run 的 ACK / 设置轮 / 新 run 的通知顺序、
就地调并发、`unchanged`、`not_found`、`not_configurable`、`model_unavailable`、忙会话下的延迟落行，
两侧 ACK 与行投影逐字一致）。
