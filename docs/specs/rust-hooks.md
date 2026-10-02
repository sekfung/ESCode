# Rust runtime：Hooks（生命周期钩子）

> 状态：H1 / H2 / H3 已实现（2026-10-03），默认仍为 Node runtime。

## 背景

Node runtime 的 hooks 子系统约 4500 行，覆盖：配置合并（用户 `~/.zcode/cli/config.json`、env、CLI 覆盖、插件
`hooks/hooks.json` / `manifest.hooks`）、按事件与 matcher 选取、`command` / `process` 两种执行、stdin JSON 协议与
exit code 2 阻断、`HookJSONOutput` 校验与聚合（权限决定、改写入参、追加上下文、阻止继续、Stop 续跑）、生命周期事件
（`HookRun*`）→ V4 `hookInvocation` 行，以及工作区（项目）hooks 的信任审核（review 交互、`workspace/hooks/trustGrant`、
信任库、准入策略）。Rust runtime 目前没有任何 hooks 支持。

## 路线（与动态工作流同一性价比判断）

钩子的语义面（配置口径、聚合规则、输出 schema、插件变量展开、准入）全部是 TS 实现细节，逐行移植成本高且极易漂移；
钩子本身是外部进程，执行开销远大于一次本地 IPC。因此：

- **运行器复用 TS**：工作流宿主（`__zcode-workflow-host`，同一个 Node 子进程）新增 hooks 面——按会话加载与 Node 相同的
  运行时 hooks 配置（`createConfig` + 插件 hook 来源 + `mergeRuntimeHooks`），用 core 的 `createConfiguredHookRunner`
  执行；生命周期事件实时通知 Rust。只有配置里存在 hooks 时才拉起宿主。
- **调用点与投影在 Rust**：SessionStart / UserPromptSubmit / PreToolUse / PermissionRequest / PostToolUse /
  PostToolUseFailure / Stop 的调用时机、结果的模型面效果（追加上下文的 system reminder、拒绝文案、改写入参、续跑）、
  以及 `hookInvocation` 行投影由 Rust 按 TS 逐条对齐。

## 分期

- **H1**：用户与插件 hooks。宿主 `hooks.load` / `hooks.run`，生命周期通知；Rust 调用点 + 行投影；差分覆盖每个事件。
- **H2**：工作区（项目）hooks 与信任审核：review 交互、`workspace/hooks/trustGrant`、准入（`workspace-hook-runtime-admission`）、
  信任库持久化。
- **H3**：会话 mailbox 内部 hooks（`ZCODE_MAILBOX_ROOT` 灰度）。

## H1 进度

- 宿主 `hooks.run {session, cwd, input}`：按会话以 `resolveRuntimeHooks`（用户 / env / 插件，与 Node 启动同一口径；默认
  `hooks.enabled: false`）装配 `createConfiguredHookRunner`；补 `cwd` / `timestamp` / PostToolUse `toolResultPreview`；
  生命周期事件以 `hookEvent` 通知（带事件时间戳 `at`）。Rust 先做存在性判定（用户配置有事件或插件声明 hook 来源），
  宿主确认无运行器后本会话不再拉起宿主。
- 已接：PreToolUse（预处理之后、权限之前；deny / preventContinuation → 权限失败 + 追加上下文；updatedInput 改写并重校验；
  allow / ask 与权限判定合并，alwaysAsk 不被 allow 抹掉）、PostToolUse / PostToolUseFailure（追加上下文接在模型内容后）。
  `hookInvocation` 行投影（`hook_rows.rs`，TS `onHookRunLifecycle`）。差分：`zcode-cli-rust-hooks-tool.test.ts`。
- PermissionRequest：确认窗挂起后起 hook 链，结论经 Host 通道回 owner，与用户应答竞速（先到者生效，同一套行 / 遥测收口）；
  hook 无结论则退赛。差分：`zcode-cli-rust-hooks-permission.test.ts`。
- 会话级：SessionStart（本进程内每会话首轮一次；历史里已有输入记为 resume）与 UserPromptSubmit（只在真实用户输入轮）
  在首个模型请求前运行，追加上下文以 `hook_context` system reminder 插在本轮输入（及其 referenced / date / mode 提醒）之前；
  UserPromptSubmit 阻止继续时撤回本轮输入消息、不请求模型（行与 `fault.runtime.hookBlocked` 错误由投影给出）。Stop 在纯文本
  收尾时运行，`stopShouldContinue` 且有上下文时追加并续跑同一轮（至多 3 次）。插入 / 撤回消息走整段重写落库。
  差分：`zcode-cli-rust-hooks-turn.test.ts`。

## H2 进度

- 宿主按会话装配 bootstrap `createWorkspaceHookRuntimeSecurity`（与 app-server 同配置：trust 开启、宿主级 policy provider、
  审核宿主上下文），运行器带工作区准入；SessionStart 前 `admission.activate(source)`。准入 / 审核事件（`workspace_hook_*`）
  经 `hookEvent` 通知 Rust，投影成 `workspaceHookAdmission` 状态（运行期，不落库）与 `workspaceHookReview` 待处理交互
  （同 flow 更高 generation 才替换）。V4 `respond/toggle/revoke/requestWorkspaceHookReview` 与 `workspace/hooks/trustGrant`
  转宿主（`hooks.review` / `hooks.trustGrant`，授权成功重载同工作区会话）。
  差分：`zcode-cli-rust-hooks-workspace.test.ts`（软门禁 → 审核 → 信任 → 执行；过期授权拒绝）。

## H3 进度

- `ZCODE_MESSAGE_ENABLED` 灰度下宿主注册 core `createSessionMailboxHookRegistrations`（`ZCODE_MAILBOX_ROOT`，默认
  `~/.zcode/mailbox`）：UserPromptSubmit / Stop 的未读消息作为追加上下文；PostToolUse 取到的消息经 `mailboxGuide` 回 Rust，
  在当前轮作为 guide 输入（clientId `session-mailbox`，原文进历史、不套中途消息包装；轮次已变则丢弃）。
  差分：`zcode-cli-rust-hooks-mailbox.test.ts`。

## 已知差异

- PreToolUse 的 `riskLevel` / `sideEffectScope` 取生成的静态工具元数据；MCP 工具没有该元数据（TS 取 MCP 条目的元数据）。
- PostToolUseFailure 的 `error.type` 固定为 `ToolExecutionFailed`，`isInterrupt` 固定 false（TS 区分取消 / 超时）。
- PermissionRequest 的 `decision.updatedInput`（改写入参后需按新入参重判权限）暂未支持，按退赛处理；用户先应答时
  hook 进程不被中止（TS abort 败者），其迟到结论被忽略。
- TS 在会话 resume 时即跑 SessionStart(resume)；Rust 推迟到该会话本进程内的首轮。
- UserPromptSubmit 的 `attachmentsSummary` 未提供。
- 工作区审核交互随会话落库；TS 在 Runtime 重启（SessionResumed）时清掉旧审核与提示条，Rust 冷恢复后旧审核可能残留到
  宿主重新上报。
- 回合之外到达的 hook 事件暂不投影行（TS 挂起到下一回合）。
