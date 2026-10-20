# Rust 阶段 2：RPC 事件面 + `escode-agent` V4 连接作用域（rust-unified-host.md 子 spec）

> 状态：立项 · 2026-10-15
> 父计划：`rust-unified-host.md` §阶段 2（优先序 5 会话与任务域的前置通路）；兄弟子 spec：`rust-u2-host-skeleton.md`（通路/队列表）、`rust-u2-tasks-index.md`（任务域）。
> 立项原因（2026-10-15 评审实测，不是文档勾选项）：Renderer 的**对话主链路**（V4 conversation 订阅、命令、帧推送）
> 全部经 `escode-agent` channel（`packages/ui/src/v4/agentConversationTransport.ts` → `IESCodeAgentService` 的
> `*V4` 方法 + `onDynamicConversationFrame`），而 Rust Host 既没有该 channel，也没有 RPC 事件面
> （`EventListen` 回 NotImplemented）。dev 开关下打开会话后无法流式对话；之前的交接手册与契约清单均未列出该 channel。

## 1. 范围

| 层                               | 交付                                                                                                                                                                                                                                          | TS 真相源                                                                                                                                                        |
| -------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| RPC 会话（host-rpc）             | 并发请求处理、`PromiseCancel` 真取消、`EventListen`/`EventDispose`/`EventFire`、连接上下文（connectionId/clientMode）、连接级出站队列 + 字节水位                                                                                              | `packages/rpc/src/channelServer.ts`（`onPromise`/`onEventListen`/`disposeActiveRequest`）、`proxy-channel.ts`（`onX` 静态事件与 `onDynamicX(arg)` 动态事件约定） |
| runtime 实例（host-runtime）     | 反向请求应答（runtime→Host request）、实例退出时在飞请求立即失败、订阅者溢出 fail-closed、退出/重启生命周期事件                                                                                                                               | `escodeAgentService.ts` 的 `client.onRequest`（兜底 `-32601 Unsupported ESCode Protocol request: <method>`）、`onAgentRuntimeLifecycle`                          |
| `escode-agent` V4 面（host-rpc） | hello/initialize 握手；conversation / sessions-index / workspace-config 三类 topic 的 subscribe/unsubscribe/resync 与帧路由（ACK 前暂存）；`sendConversationCommandV4`、`queryConversationCommandsV4` 与只读 query 族、附件事务；连接关闭清理 | `escodeAgentConnectionScope.ts`（全文）、`escodeAgentService.ts:5850-6440`（base 层 V4 转发）                                                                    |

不在本片：legacy `escode-agent` 方法（插件/自动化/MCP UI/wiki 等 112 个方法中非 V4 的部分，随各自域）；
mobile `web-remote-replayable` attachment 的**接入**（本片在连接上下文里支持 clientMode，但 Main 仍把手机接到 Node Host，见 §6）。

## 2. 状态所有者与事件顺序

```
Renderer(port)                Rust Host                                      runtime 实例（每 workspace 一个）
   │  helloConversationV4 ──►  ConnectionScope（每 RPC 连接一个）
   │  initialize(clientHello)   - 握手态、boundClientId、workflowRunDeltas
   │                            - owned: (kind, workspaceKey, topic, connectionId) → subscriptionId
   │                            - pending: ACK 前暂存帧（≤1024 帧 / ≤32 MiB，溢出整批失败）
   │  onDynamicConversationFrame(ws) ──► 登记 EventListen id → routed emitter
   │  subscribeConversationV4 ──► begin pending ──► v4/conversation/subscribe ─────► 订阅登记（runtime 投递层是事实源）
   │                                                         ◄──── v4/conversation/frame（可能先于 ACK 到达）
   │                                     pending 暂存 ◄──┘
   │                            ◄── ACK ◄────────────────────────────────── result.ack.subscriptionId
   │                            remember owner → 按到达序释放暂存帧
   │  ◄── EventFire(frame) ◄── 只投递 owned (subscriptionId, topic) 的帧
   │  sendConversationCommandV4 ► 校验 clientId == boundClientId，注入可信连接 ► v4/command
   │  (port close) ──────────► dispose：flow closed → 逐个 unsubscribe（allSettled）
```

所有者：

- **runtime**：订阅、投递、快照恢复、命令幂等（不变）。
- **Host ConnectionScope**：只持连接级「谁拥有哪个订阅」的路由事实，不缓存帧、不改帧内容。
- **Host 实例表**：runtime 代次；实例退出发布 `unavailable(workspaceKey, generation)`，ConnectionScope 据此丢弃本地 owner（**不**向新实例反向 unsubscribe 同名订阅）与在飞 ACK。

幂等与边界：

- 可信字段（`connectionId`/`clientMode`/`workflowRunDeltas`/`deliveryProfile`/`subscriberScope`）只由 Host 写入，Renderer 入参中的同名字段一律剥除（TS `withTrustedConnection`）。
- `unsubscribe` 找不到 owner 时静默成功；`resync` 找不到 owner 回 `fault.subscription.notOwned`；同一 (kind, workspace, subscriptionId) 多重匹配时拒绝猜测。
- 连接关闭与迟到 ACK 并发：迟到 ACK 不登记、立即反向 unsubscribe，并回 `fault.connection.closed`。

## 3. 接口

### 3.1 RPC 会话

- 每条请求独立任务执行；响应经连接出站队列写回（顺序 = 完成序，与 TS Promise 语义一致）。
- `PromiseCancel(id)`：中止该请求任务，不回响应（TS 侧客户端已自行 reject）。
- `EventListen(id, channel, event, arg)`：channel 的事件表命中即建立订阅；每个事件值回 `[204, id] + data`。
  未知 channel/事件：TS 抛 `Event not found`（服务端日志，客户端无响应）；Rust 同样不回响应，只记 warn。
- `EventDispose(id)`：取消订阅（丢弃事件流句柄）。
- 连接关闭：取消全部在飞请求与事件订阅，再按注册顺序执行各 channel 的连接级 `dispose` 钩子。
- 出站队列字节水位：高水位 8 MiB → 连接 `saturated`；低水位 2 MiB → `drained`；硬上限 64 MiB → 关闭连接（fail closed，
  不静默丢帧）。水位边沿交给连接作用域（V4 面把它转发为 `v4/connection/flow`，TS `setTransportFlowState` 同义）。

### 3.2 连接上下文与访问控制

`ConnectionContext { connection_id: "host-rpc-<uuid v4>", client_mode, flow }`。`client_mode` 由**连接路径**决定（与 TS
`server-core/http.ts` 同）：`/ws/host` = `desktop-continuous`（Main 桥接的桌面 Renderer），`/ws` = `web-remote-replayable`。
Renderer 不能自选。

访问控制（评审 A9；2026-10-15 前 `/ws` 免鉴权且 Main 正是把桌面 Renderer 桥到 `/ws`）：

- 带 `Origin` 头的请求一律 403（浏览器来源：跨源 WebSocket 不受同源限制、DNS rebinding）；
- 两条 WS 路径都要一次性 capability（`x-escode-rpc-host-capability`，CSPRNG，TTL 60 s，用后即焚）；
- `POST /api/rpc-host-capability` 与 `GET /api/server-info` 要进程密钥头 `x-escode-host-secret`；密钥在 `rpcListen` 后经控制通道
  （`rpcListening.secret`）交给 Main，不出现在任何网络响应里；
- Main 侧：`issueRustHostCapability` → `ws://addr/ws/host`（`rustHostBridge.ts`），申请期间到达的端口消息按序排队补发。

### 3.3 runtime 反向请求

| 方法                                | Host 应答                                                                                                                                                                                                                                                                                                                                       |
| ----------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `session/requestRuntimePreferences` | 本地权威（TS `resolveSessionRuntimePreferences`，`node.ts:2217`）：读 settings → `{askUserQuestionAutoResolutionEnabled: !== false, nativeSearchEnhancementsEnabled: !== false, memoryEnabled: === true, modelContextBudgetStrategy: "preflight-v1", integratedTerminalShell?（仅 scope=user-execution 且有值）, subagentRuntimeConfigEnabled}` |
| 其余                                | `-32601 Unsupported ESCode Protocol request: <method>`（TS 兜底同文）                                                                                                                                                                                                                                                                           |

`subagentRuntimeConfigEnabled`：TS 在本地 workspace 为 `true`（Host 能回 `subagents/readRuntimeConfig`）；subagents 域未迁移前
Rust 回 `false`（登记差异 §6-2）。

### 3.4 `escode-agent` 方法表（本片）

`helloConversationV4`、`initializeConversationV4`、`subscribeConversationV4`、`unsubscribeConversationV4`、`resyncConversationV4`、
`subscribeSessionsIndexV4`、`unsubscribeSessionsIndexV4`、`resyncSessionsIndexV4`、`subscribeWorkspaceConfigV4`、
`unsubscribeWorkspaceConfigV4`、`resyncWorkspaceConfigV4`、`sendConversationCommandV4`、`queryConversationCommandsV4`、
`conversationRowsRangeV4`、`conversationPlansV4`、`conversationFileChangesV4`、`conversationFileRewindPreviewV4`、
`backgroundBashOutputV4`、`conversationWorkflowRun{Events,s,Artifacts,ArtifactData,ArtifactRead,Workspace,NodeResult}V4`、
`attachment{Begin,Chunk,Commit,Abort,Read,PreviewSource}V4`、`conversationAttachment{Read,Stat}V4`；
动态事件 `onDynamicConversationFrame`、`onDynamicSessionsIndexFrame`、`onDynamicWorkspaceConfigFrame`；
静态事件 `onAgentRuntimeLifecycle`。

## 4. 验收

1. RPC 会话：慢请求不阻塞同连接其他请求（并发）；`PromiseCancel` 中止；事件 `EventFire` 帧形与 TS `ChannelClient` 互通
   （`packages/desktop/test/rustHostRpc.test.ts` 用真实 `@escode/client` 订阅）；连接关闭触发 dispose 钩子。
2. 实例退出：在飞 `call` 立即失败（不等 30 s 超时）；订阅者溢出时接收端可观测（lagged），不静默丢帧。
3. 反向请求：真实 embedded 实例跑一轮 `sendText`，`runtimePreferences` 被应答、回合到达终态（此前该路径无人应答）。
4. V4 面端到端（真 WebSocket + 真 embedded 实例 + fixture 模型）：hello → initialize → `onDynamicConversationFrame` 监听
   → `sendConversationCommandV4(createSession)` → `subscribeConversationV4` → 收到首帧快照（ACK 先于首帧投递）；
   `clientId` 不一致的命令被拒；未握手调用被拒；连接关闭后 runtime 侧订阅被注销。
5. 负向验证：去掉 ACK 前暂存（直接丢早帧）、去掉可信字段剥除，相应用例必须失败。

## 5. 实施进度

| 切片                                                                                                  | 状态 | 证据                                                                                                                                                                                                                                                                                                                                                                                                         |
| ----------------------------------------------------------------------------------------------------- | ---- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| runtime 实例层：反向请求应答、退出即失败、订阅溢出可观测、生命周期事件、早期帧缓冲、child stderr      | 完成 | `host-runtime/tests/host_io.rs`（5 例，含负向验证）                                                                                                                                                                                                                                                                                                                                                          |
| RPC 连接层：并发请求、真取消、EventListen/EventFire/EventDispose、连接级 channel + 关闭钩子、出站水位 | 完成 | `host-rpc/tests/rpc_connection.rs`（3 例，负向：去掉 abort 后取消与注销用例失败）                                                                                                                                                                                                                                                                                                                            |
| 访问控制（A9）：Origin 403、两路径 capability、进程密钥、CSPRNG 令牌                                  | 完成 | `host-rpc/tests/rpc_surface.rs`、`packages/desktop/test/rustHostBridge.test.ts` / `rustHostRpc.test.ts`（真实客户端）                                                                                                                                                                                                                                                                                        |
| 反向请求本地偏好（`session/requestRuntimePreferences`）                                               | 完成 | `host-rpc/src/runtime_requests.rs` 单测 3 例；真回合端到端随 V4 面                                                                                                                                                                                                                                                                                                                                           |
| `escode-agent` V4 连接作用域（34 方法 + 3 动态事件 + 2 静态事件）                                     | 完成 | `host-rpc/tests/rpc_agent_v4.rs`（真 embedded 实例 + mock 模型：一轮真实对话帧流、握手/clientId/归属/dormant 断言；负向：恢复「反向请求无人应答」后回合停滞 30.2 s 被延迟断言拦下）、`agent_v4::scope_routing` 单测 3 例（ACK 前暂存保序、溢出显式失败、换代迟到 ACK 不反向注销；负向：关掉暂存后 2 例失败）、`packages/desktop/test/rustHostAgentV4.test.ts`（真实 `@escode/client` 收静态/动态 EventFire） |
| 契约清单全量化（评审 A13）                                                                            | 完成 | `scripts/generate-escode-host-service-inventory.mjs`（TS 编译器 API 解析 51 个 descriptor / 619 成员，`--check` 进 `pnpm check:escode-host`）；`host-proto` 只登记已实现成员；`contract.rs` 按生成全集对账                                                                                                                                                                                                   |
| 模型请求代理（评审 A15，CLI `crates/net`）                                                            | 完成 | 未解析出代理即直连（与 TS 同），`crates/net/tests/model_direct.rs`（负向：恢复旧行为即失败）                                                                                                                                                                                                                                                                                                                 |

## 6. 登记差异

1. **replayable attachment 的调用面变更**：连接上下文保留 `web-remote-replayable`，当前用于远程 workspace attachment；远程 workspace已于 2026-10-20 删除，不再作为调用方。
   移除条件：remote 域（优先序 9）落地。
2. **`subagentRuntimeConfigEnabled=false`**：自定义子代理运行时配置不下发。可见条件：dev 开关下用户自定义 subagent 不生效。
   移除条件：subagents 域实现 `subagents/readRuntimeConfig`。
3. **不做账号 provider 同步屏障**（`ensureAccountProviderConfigSynced`）：providers/account 域未迁移；Rust Host 下模型配置由 Main 经
   `RuntimeOptions.config` 提供。
4. **`openTiming` 只透传 runtime 字段**：TS 在 ACK 上追加 Host 侧计时（hostPrepareMs/cliBootstrapMs/…，诊断用）；Rust 透传 runtime 的
   `openTiming`，Host 计时字段登记为后续项。
5. **冷订阅的 `resumeThoughtLevel` hint**：TS 从 tasks-index 读 `thoughtLevel` 注入；Rust 同样读（索引可用时），索引不可用时省略（TS 同为失败即省略）。
6. **工具面灰度（动态工作流）fail-closed**：TS 由服务端 client-config 判定后注入 `createSession` 信封；对应域未迁移前 Rust 一律不注入（缺省即关闭），**不**信任 Renderer 入参。
7. **浏览器环境上下文**（`collectBrowserAmbientContext`）与 TTFT 本地观测（`onDynamicLocalTtftFacts`）：浏览器控制域未迁移，`sendText` 不附 `browserAmbientContext`；`ttft` 字段对非桌面本地连接剥除（与 TS 同）。
8. **帧形状校验**：TS 用 zod wire candidate 校验并丢弃非法帧；Rust 只做路由所需的最小校验（`topic`/`subscriptionId` 为字符串），形状由 Renderer 解码器把关。可见条件：runtime 发出形状非法的帧时，TS 丢弃、Rust 透传给 Renderer 由其拒绝。
9. **静态事件不做 `bufferEvent`**：TS `ProxyChannel.fromService` 把 `onX` 事件缓冲到首个监听者；Rust 只投递监听之后的事件。可见条件：Renderer 在实例启动之后才监听 `onAgentRuntimeLifecycle` 时收不到更早的 `available`（Renderer 现有用法在连接建立后立即监听）。
10. **水位只转发给已存在的实例**：TS 可能为 flow 拉起 runtime；Rust 用 existing-only（实例已退出时其订阅随之消失，无需通知）。
11. **无 trusted-host-relay 角色**：Rust Host 的连接一律 terminal-client（`setConnectionFlowStateV4` 回 `fault.connection.flowControlForbidden`，与 TS terminal-client 同）；relay 场景随 remote 域。
12. **本地媒体预览授权**（`authorizeLocalMediaPreviewPath`，media-preview 域）未迁移：桌面本地连接拿到 `local_path` 时与 TS 缺授权器同错 `fault.attachment.previewPathAuthorizationUnavailable`。
13. **clientHello 校验失败的文案**：TS 抛 zod 的 issue JSON；Rust 回 `ZodError` + 简述（Renderer 不解析该文案）。
14. **生命周期事件的 `workspaceIdentity`**：取实例装配时的身份（Main 的 options factory 总带身份键）；TS 只在入参带 identity 时才有该键。
15. **Provider runtime headers 的账号合成**：Rust Host 已接反向请求/应答（`onDynamicWorkspaceProviderRuntimeHeadersRequest` + `respondProviderRuntimeHeaders`），
    但把 Renderer 的 `runtimeProviderHeaders` 直接作为 `requestAuth.headers`，不做 TS 的账号 IO；
    `onDynamicWorkspaceProviderRuntimeHeadersCancelled` 仍 Pending（取消通知未从实例帧路由到注册表），超时 120s 后按 `headersApplied:false` 收口。
    移除条件：接账号请求材料与取消事件路由。
