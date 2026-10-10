# Rust Cron 工具（CronCreate / CronList / CronUpdate / CronDelete）

2026-09-25。用户决定实现（rust-release-rollback.md「功能缺口范围」）。App 的定时任务由 Host 持有（调度、持久化、派发），
runtime 只做入参校验、守卫与协议转发；逐条对齐 TS `core/src/tool/handlers/cron.ts` 与
`bootstrap/src/escode-protocol/automation-port.ts`、`escode-protocol-v4/commands/prompt-turn.ts`。

## 所有者与流程

```mermaid
sequenceDiagram
  participant H as Host（automation service）
  participant E as Session owner（Engine）
  participant L as agent_loop（cron_tool）
  H->>E: v4 sendText（automationId / toolDisallowlist / botDeliveryTarget）
  E->>E: admission 记录本轮事实（automationId 缺省时取 automation- 前缀的 commandId）
  E->>L: 运行；本轮禁用工具从模型可见工具面剔除
  L->>L: 自动化轮拒绝写工具；入参按 TS schema 校验（trim）
  L->>E: Event::HostRequest(automation/checkTaskBinding …)
  E->>H: 反向请求；按 id 回填结果或错误（-32601 回落 automation/list）
  L->>E: Event::HostRequest(automation/create|update|list|delete)
  E-->>L: 结果（原始 JSON 文本，保持键顺序）
  L->>E: CronCreate 成功后冻结会话标题（custom）
```

- 本轮事实：`automationId`（显式或 commandId 以 `automation-` 开头时取 `:` 之前部分）、`toolDisallowlist`
  （自动化轮额外加入 CronCreate/CronUpdate/CronDelete）、`botDeliveryTarget`；admission 时写入该输入的 payload，
  运行开始时读出。自动化轮（有 automationId 或禁用列表包含全部三个写工具）调用写工具时拒绝。
- 反向请求：Engine 通用 `HostRequest`，以请求 id 路由应答；错误保留 `code` 与 `message`。
- CronCreate：本轮已由 automation 派发时拒绝；先查当前会话是否已绑定定时任务（查询失败即拒绝，`-32601` 回落列表判定）；
  参数映射与 TS 相同（相对延迟、间隔 carrier、有限次数、会话模型与模式 `auto→build`、`targetTaskId`、bot 回推地址）；
  Host 返回创建上限错误（`AUTOMATION_CREATE_LIMIT_REACHED`）时给模型固定文案并结束本轮后续工具。
- 输出与模型可见内容：`JSON.stringify(output)`，automation 字段按 TS `toModelAutomation` 顺序，嵌套 `scheduleRule` 保持 Host 原始键顺序。
- 已知差异：Node 以 strict 协议 schema 校验 Host 应答（多余字段即报错），Rust 不校验应答形态；同版本 Host 不会产出非法应答。
- OffPeak 字段（`offPeakTaskId` / `offPeakRunType`）见 rust-offpeak.md（2026-09-27 用户决定原生实现）。

## refine 文案（2026-09-27）

- 差分发现：入参只违反 refine 时，Node 的模型可见错误是 handler 抛出的 ZodError 文案，即全部失败 issue 的
  `JSON.stringify(issues, null, 2)`（经错误文案规整折叠空白）。zod 按声明顺序执行每条 refine，失败不中断后续 refine。
  Rust 之前只报第一条，且文案是自拟缩写。
- 规则：`cron_input::refine` 按 TS 声明顺序逐条判定，输出 `{code:"custom", message, path}` 列表，文案与 path 逐字取自 TS；
  CronUpdate "至少一个字段" 的 path 为空数组。
- 验收：`cron_corpus.json` 的失败用例中只含 refine 问题的，记录 TS `message`，Rust 单测逐字比对；语料含多条同时失败的用例。

## 验收

- `scripts/generate-escode-cli-rust-cron-corpus.mjs`：真实 handler + 真实协议端口 + 脚本化 Host 的 21 个流程与 39 个校验用例，
  Rust 逐条比对请求序列与参数、输出、模型可见文案、错误、创建上限与标题冻结。
- App 差分：同一段对话在 Node 与 Rust 上创建/列出/更新/删除定时任务，Host 收到的请求与模型看到的结果一致；
  自动化轮中写工具不可见且被拒绝。
