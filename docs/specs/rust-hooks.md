# Rust runtime：Hooks（生命周期钩子）

> 状态：设计 / H1 进行中（2026-10-03）。

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
- 待接：PermissionRequest、UserPromptSubmit、SessionStart（含 TS 的 pending 挂到下一回合）、Stop（续跑）。

## 已知差异

- PreToolUse 的 `riskLevel` / `sideEffectScope` 取生成的静态工具元数据；MCP 工具没有该元数据（TS 取 MCP 条目的元数据）。
- PostToolUseFailure 的 `error.type` 固定为 `ToolExecutionFailed`，`isInterrupt` 固定 false（TS 区分取消 / 超时）。
- 回合之外到达的 hook 事件（SessionStart 之前的启动钩子）暂不投影行。
