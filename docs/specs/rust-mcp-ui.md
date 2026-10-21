# Rust runtime：MCP Apps（`mcp/ui*`）迁移设计

> 状态：P1/P2 已落地（2026-10-21）
> 已实现 runtime：实例 open/validate/close、`mcp/readResource`、`mcp/uiReadResource`、
> `mcp/uiListResources`、`mcp/uiListResourceTemplates`、`mcp/uiCallTool`、`mcp/uiCancelCall`（9 个方法）。
> 已注册 Host `plugin-ui-bridge` 12/12（含 prepareSandbox、listSurfaces、订阅）。
> 仍 Pending：App 工具 4、采样 2。
> 起因：Host 契约清单里 `plugin-ui-bridge`（12）、`plugin-ui-app-tools`（4）、`plugin-ui-sampling`（2）
> 共 18 个成员仍为 Pending。它们的共同前置是 Rust runtime 没有实现任何 `mcp/ui*`；
> 只迁 Host 会把显式 `NotImplemented` 换成运行时的 `Unsupported method`，不能算完成
> （[rust-u2-host-scope-boundary.md](./rust-u2-host-scope-boundary.md) §1.1）。
> TS 事实源：`apps/escode-cli/packages/bootstrap/src/escode-protocol/mcp-ui/`（约 2.4k 行），
> 共享 schema：`packages/shared/src/escode-protocol/index.ts:588-810`，
> 共享常量：`packages/shared/src/mcp-apps/`。

## 1. 范围

### 1.1 runtime 方法

| 方法 | 作用 | 依赖 |
| --- | --- | --- |
| `mcp/uiOpenInstance` | 为一次页面打开签发实例凭证（runtimeId / generation / token / appIdentity） | 实例登记表 |
| `mcp/uiValidateInstance` | 校验凭证与当前连接代际；失效即撤销 | 实例登记表 + MCP 连接快照 |
| `mcp/uiCloseInstance` | 撤销凭证；`onlyIfIdle` 与调用 pin 原子裁决 | 实例登记表 + 调用登记表 |
| `mcp/uiRecycleInstance` | 旧实例闲置时换发新代际（Host `recycleInstance` 使用） | 实例登记表 |
| `mcp/readResource` | Host 取 `ui://` 资源（保留 `_meta`） | MCP `resources/read` |
| `mcp/uiReadResource` | 页面读资源；去掉 `_meta`，8 MiB + MIME 白名单 | MCP `resources/read` |
| `mcp/uiListResources` | `resources/list` 代理（标准描述符） | MCP `resources/list` |
| `mcp/uiListResourceTemplates` | `resources/templates/list` 代理 | MCP `resources/templates/list` |
| `mcp/uiSubscribeResource` / `mcp/uiUnsubscribeResource` | 资源更新订阅；通知带完整凭证 | MCP `resources/subscribe` + 连接通知 |
| `mcp/uiCallTool` | 页面调用本 server 可见工具；复用工具权限链，不产生 transcript row | MCP `tools/call` + visibility |
| `mcp/uiCancelCall` | 按 `callId` abort 进行中的页面调用 | 调用登记表 |
| `mcp/uiRegisterAppTools` / `mcp/uiUnregisterAppTools` | App-Provided Tools 登记/注销（按实例整体替换） | App 工具登记表 |
| `mcp/uiClaimAppToolCall` / `mcp/uiResolveAppToolCall` | 认领与回传（5 s 认领、30 s 执行、只执行一次） | App 工具登记表 |
| `mcp/uiSampling` / `mcp/uiCancelSampling` | 页面采样；60 s 绝对期限、并发上限 1/4/8 | 采样账本 + 任务模型 |
| `plugins/listUiSurfaces` | 工作区级 MCP App 面板枚举 | 资源发现 + 插件目录 |

`mcp/uiCancelCall` 是 Rust 新增的取消面（TS 已有）；Host 的 `plugin-ui-bridge.cancelToolCall` 依赖它。

### 1.2 Host 成员

| channel | 成员 |
| --- | --- |
| `plugin-ui-bridge` | `prepareSandbox`、`readResource`、`callTool`、`cancelToolCall`、`listResources`、`listResourceTemplates`、`subscribeResource`、`unsubscribeResource`、`listSurfaces`、`validateInstance`、`recycleInstance`、`closeInstance` |
| `plugin-ui-app-tools` | `registerAppTools`、`unregisterAppTools`、`claimAppToolCall`、`resolveAppToolCall` |
| `plugin-ui-sampling` | `sample`、`cancelSampling` |

**不做**：`cua-permission`（2）与 `cua-pip-session`（3）不属本 spec；
`provider-provisioning-target.apply`（1）无消费者，维持非缺口。

## 2. 所有者与状态边界

```text
Host RPC（plugin-ui-*）
      │  forward::runtime_call
      ▼
Rust runtime Engine（core crate）
      │  ToolPort（新增默认不可用的 mcp_ui_* 方法）
      ▼
WorkspaceTools（tools crate）
      │
      ├─ McpUi 状态（本 spec 新增）
      │    ├─ instances：实例凭证唯一 owner
      │    ├─ calls：页面工具调用登记（callId → AbortHandle）
      │    ├─ app_tools：会话内登记表 + 待执行调用信箱
      │    └─ sampling：操作账本 + 并发额度
      │
      └─ Hub（既有 MCP 连接/工具绑定）
           ├─ resources/list|read|templates|subscribe
           ├─ tools/call（复用既有 connection.call）
           └─ server→client sampling 回调
```

规则：

1. **实例凭证只在 runtime 内铸造**。Host 只保存并原样回传；不解释 `token`，不自行比较 `runtimeId`。
2. **每个 `(workspace, sessionId, pluginId, serverName, scopeId, resourceUri, ownerWebContentsId)`
   的一个逻辑页面只有一个有效实例**。同连接同代的重复 open 幂等返回同一凭证；连接代际变化即撤销。
3. **调用登记表是取消与终态去重的唯一 owner**。`callId` 接纳后不可重放；取消不回滚外部已发生的操作。
4. **App 工具登记表按实例整体替换**。同 server 只有序号最大的实例暴露给模型；认领后 5 s 内必须执行，
   30 s 绝对期限；执行只发生一次。
5. **采样账本按实例绑定连接代际**。先 pin 后执行；实例失效立即清账本，旧 `operationId` 永久拒绝。
6. 所有页面方向的状态都是 **live-only**：不落 SQLite、不进 snapshot/replayable、不改 CommandInbox。

### 2.1 事件顺序

```text
open        → 撤销旧代际（若逻辑键相同且连接代际变化）→ 签发新凭证 → 返回
validate    → 查 token/runtimeId/generation/appIdentity → 查连接快照代际 → 失效则撤销并报错
callTool    → 归属校验 → visibility 校验 → 登记 callId → MCP tools/call → 终态 → 注销 callId
cancelCall  → 归属校验 → abort callId（若存在）→ cancelled 布尔
close       → 从登记表移除 → abort 全部调用 → 注销资源订阅 → 释放 App 工具
sampling    → 账本容量/并发校验 → pin 实例 → 模型调用 → 校验实例仍有效 → 终态
```

## 3. 错误映射

| 条件 | JSON-RPC code | 携带 data |
| --- | --- | --- |
| server 不在本会话 MCP 配置 | `-32602` | `{serverName}` |
| 工具不存在 / 不可见给页面 | `-32602` | `{serverName, toolName}` |
| 实例凭证失效 | `-32000` | 无 |
| 资源超 8 MiB / MIME 不在白名单 | `-32001` | `{uri, bytes?, maxBytes?, mimeType?}` |
| 资源订阅不支持 | `-32000` | `{serverName, method}` |
| 采样忙碌 / 超时 / 取消 | `-32000` | 无 |
| 重复 `callId` / `operationId` | `-32602` | 无 |

## 4. 分阶段实现

### P0：rmcp 能力补齐（前置）

- `Connection` 增加 `resources/list`、`resources/templates/list`、`resources/read`、
  `resources/subscribe`、`resources/unsubscribe` 的通用请求方法。
- `Connection` 的 client handler 从 `ClientConfig` 换成自定义 `ClientHandler`，保留 `get_info()`，
  增加：
  - `create_message`（server→client sampling）回调；
  - `on_resource_updated` 回调。
- 工具发现保留 `_meta.ui.visibility`，用于页面调用资格判定。

验收：Rust 单测用 stdio fixture server 覆盖四类资源方法与一个 sampling 请求。

### P1：实例与资源（6 个 Host 成员）

- `McpUi` 实例登记表与 `open/validate/close/recycle`。
- `mcp/readResource`、`mcp/uiReadResource`、`mcp/uiListResources`、`mcp/uiListResourceTemplates`。
- Host：`prepareSandbox`（Host 组装壳 + `mainRequest` 注册）、`readResource`、`listResources`、
  `listResourceTemplates`、`validateInstance`、`closeInstance`、`recycleInstance`。

验收：MCP Apps fixture 页面能打开并读取自己的 HTML；错误路径覆盖超限、MIME、归属失败。

### P2：工具调用（3 个成员）

- `mcp/uiCallTool`、`mcp/uiCancelCall`。
- visibility（`_meta.ui.visibility`）与 fail-closed 归属。
- Host：`callTool`、`cancelToolCall`、`listSurfaces`。

验收：页面调用成功/取消/重复 callId/不可见工具四条路径与 TS 行为一致。

### P3：App-Provided Tools（4 个成员）

- 登记表、同 server 最新实例暴露、认领/回传状态机、live-only 信箱事件（Host 负责投影）。
- Host：四个成员。

验收：认领超时、重复 resolve、实例替换、会话关闭四条路径。

### P4：采样（2 个成员）

- 采样账本 + 并发额度 + 60 s 期限。
- 通过 `ModelPort` 使用任务模型；不新增审批/会话/历史。
- Host：`sample`、`cancelSampling`。

验收：成功、取消先于请求、超时、并发上限、实例失效五条路径。

## 5. 已知差异与边界

1. `plugin-ui-bridge.prepareSandbox` 仍由 Host 组装壳 HTML 并通过 `mainRequest` 调 Main 的
   `registerFromHost`；runtime 只提供 `openInstance` + `readResource`。
2. sampling 使用任务当前模型，不继承页面的会话上下文；与 TS `sampleModel` 的调用形状保持一致，
   但 Rust 侧的模型请求不产生 conversation row。
3. 资源订阅通知只走 live 通道；`web-remote-replayable` 下按既有恢复语义重建订阅，不补历史通知。
4. `plugins/listUiSurfaces` 的枚举顺序按 server 名字节序，与 `mcp_config_order` 的 JS 键序对齐规则一致。

## 6. 验收命令

```sh
pnpm check:escode-cli-rust
pnpm test:escode-cli-rust
pnpm check:escode-host
pnpm test:escode-host
pnpm typecheck && pnpm lint
node scripts/generate-escode-host-service-inventory.mjs --check
```

契约清单只有在 runtime 方法真实可用后才把对应成员翻为 `Implemented`；
在此之前 Host 不注册这些方法（避免把显式缺口换成运行时错误）。

## 7. 未决

- sampling 的模型快照来源：使用 `Engine.config` 还是会话冻结的模型身份。P4 开工前按
  `docs/specs/rust-model-io.md` 的会话模型解析口径确认。
- 资源更新通知在实例关闭/会话关闭时的撤销顺序，P1 实现时用单测固定。
