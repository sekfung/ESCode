# Rust runtime：MCP 客户端事件总线

> 状态：S1 已实现（handler + 出口），S2–S4 待接线（2026-10-21）
> 起因：`mcp/ui*` 的资源订阅、App-Provided Tools 与 sampling 都需要 MCP server 主动通知/请求
> 进入 runtime；此前 `Connection` 用 `ClientConfig` 作为 handler，通知全部丢弃
> （[rust-mcp-ui.md](./rust-mcp-ui.md) §4 P0）。

## 1. 所有者与边界

```text
MCP server
   │ notifications/resources/updated|list_changed
   ▼
Connection 的 HubClient（tools 适配器）
   │ McpClientEvent（纯数据，unbounded send，不阻塞传输）
   ▼
Engine 的 McpClientEvents owner（core）
   │ 1. 按 (server, uri) 查订阅登记表
   │ 2. 校验实例凭证与连接代际
   │ 3. 按 session 分组
   ▼
live-only 会话事件（不进 snapshot/replayable/持久化）
```

规则：

1. **连接层不持有页面状态**。`HubClient` 只把官方 MCP 通知转成 `McpClientEvent`；
   订阅归属、实例凭证与投影都在 Engine。
2. **通知必须非阻塞**。使用 unbounded channel（通知小且低频），避免 MCP 传输被页面渲染阻塞；
   Engine 侧负责合并与丢弃。
3. **无订阅者即丢弃**。不得为没有订阅者的通知创建会话事件或持久化记录。
4. **代际失效优先**。实例撤销、连接换代、会话关闭时，订阅登记先移除再停止投递；
   迟到通知不得投递给新实例。
5. **只走 live 通道**。资源更新不进入 snapshot、replayable 恢复、relay 或 Renderer 持久状态；
   页面重新挂载后通过 `resources/read` 重新取内容。

## 2. 事件与顺序

| 事件 | 来源 | Engine 处理 |
| --- | --- | --- |
| `ResourceUpdated { server, uri }` | `notifications/resources/updated` | 查 `(server, uri)` 订阅 → 校验实例 → 每 session 一条 `PluginUiResourceUpdated` |
| `ResourceListChanged { server }` | `notifications/resources/list_changed` | 查该 server 的全部订阅 → 每 session 一条 `PluginUiResourceListChanged` |
| `Sampling`（S3） | `sampling/createMessage` | 校验实例 → 任务模型调用 → 回包 |

顺序保证：同一 server 的通知按到达顺序进入 channel；Engine 单循环处理，校验失败的事件丢弃且
不影响后续事件。

## 3. 分阶段

### S1（已完成）

- `HubClient` 实现 `ClientHandler`：`get_info`、`on_resource_updated`、`on_resource_list_changed`。
- `McpClientEvent` / `McpClientEventSink` 定义在 core-api。
- `Hub` 持 `OnceLock<McpClientEventSink>`，`ToolPort::attach_client_events` 供 Engine 登记。
- 尚未登记出口时通知直接丢弃（不伪造订阅状态）。

### S2：资源订阅

- Engine 新增 `McpClientEvents` owner：`HashMap<(server, uri), BTreeSet<Subscriber>>`，
  `Subscriber = { sessionId, scopeId, generation, leaseToken }`。
- `mcp/uiSubscribeResource` / `UnsubscribeResource`：校验实例 → 调 MCP `resources/subscribe` →
  登记/移除；同一 `(server, uri)` 的 MCP 订阅按引用计数，最后一个退订时才发 `unsubscribe`。
- 通知路由按 `reason: ping` 之外的语义直接投递；实例校验失败即静默丢弃并清理该订阅。
- Engine 投递 `PluginUiResourceUpdated` / `PluginUiResourceListChanged` live-only 事件
  （`sequenceNumber: 0` 语义由 Host 现有的 live 通道分配）。
- Host 注册 `subscribeResource` / `unsubscribeResource`。

### S3：Sampling

- `HubClient::create_message`：把 `CreateMessageRequestParams` 序列化为 Value，经事件总线发
  `Sampling { server, params, reply }`；Engine 用任务模型执行后回包；超时/无 Engine 一律
  `McpError`（explicit failure，不返回空结果）。
- 采样账本（操作去重、并发上限、60s 期限）在 Engine 侧，按 `mcp/uiSampling` 的实例凭证绑定。

### S4：App-Provided Tools

- 复用同一事件总线投递页面调用请求（live-only）；登记表与认领/回传状态机独立于订阅。
- 模型工具面注入在 Engine 的请求组装处完成，不经过连接层。

## 4. 验收

- S1：`cargo test -p escode-cli-tools` 覆盖 handler 把两类通知写入 sink；未登记 sink 时丢弃且不 panic。
- S2：订阅/退订引用计数、实例撤销后通知丢弃、会话关闭清理三条用例。
- S3：超时、取消、实例失效三条用例。
- S4：认领超时、重复 resolve、实例替换三条用例。

## 5. 已知边界

- 通知只覆盖 MCP 标准 `resources/*` 与 `sampling/createMessage`；`logging`、`tools/list_changed`
  仍按既有路径处理（日志/回合边界重拉）。
- 不新增 SQLite 表，不持久化 widgetState 或订阅关系。
