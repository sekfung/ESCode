# 远程 workspace 统一到 Source Server（Rust Host）

> 状态：S1 已随 v3.16.2 同步落地，S2（Host 侧 registry 只读面 + §9.8 写面）已落地；S3 进行中（S3-a/S3-b/S3-c-1/S3-c-2/S3-c-3a/S3-c-3b/S3-c-3c/S3-c-3d 已落地；Linux x64 真实 SSH formal E2E 覆盖首连双 workspace、App 重启恢复并原位重连、同一 workspace 双窗口共享实例、真实断开 SSH transport 后离线→原位重连并续发，4 例通过；Windows/macOS E2E 待补）；S4 进行中（S4-a 兼容/状态字段、S4-b release 解析/preflight/install preview/部署决策、S4-c 首次安装（local-upload + remote-download）/部署锁/component meta、S4-d 显式更新/running-task guard/失败回滚、S4-e `lastOnlineAt` 持久化/启动自动连接/2-4-8s 恢复预算、S4-f `source-server-workspace` 转发 + 迁移统计投影/报告/重试/放弃、S4-g Provider provisioning 首连屏障/后续触发/远端 runtime preferences 应答、S4-h known_hosts 策略（accept-new + Desktop 自有 known_hosts + host key 变化不可重试）、S4-j 握手身份记录/`identity-change-pending`/Profile 重绑定已落地；S4-i 进行中（Windows 交叉编译 + clippy 通过、平台审计完成；Windows/macOS 桌面 E2E 待对应平台环境，见 §11.11））。
> S5-a 静态完成（Rust `ensureProfile` + 首连 resolve endpoint + Main target single-flight/sourceServerId 回填已落地，Linux 单测与类型、Lint、架构、Host 边界门禁通过；真实直接首连 E2E 已通过：Linux SSH `desktop-e2e-20261031-122307-136`）；S5-b 完成（`ESCODE_RUST_HOST` 收窄为本地开关，远程路由不再读取；Node Host 对非 `kind: "server"` 远端显式 `REMOTE_RUST_HOST_REQUIRED` 拒绝并 dispose）；S5-c/S5-d 更新（2026-11-01：`packages/web` 已于 2026-10-20 随手机远控删除，仓库内已无 `/api/connect-remote` / `/ws/remote/:id` 调用方；两条 route 已删除并以 404 测试锁定，Node remote deploy 删除继续按 §12.3 收口）；S5-e 完成（远端项目存储与 `project` / `remote-projects` channel 收口到 Controller Rust Host，Source 只提供文件系统探测；真实 SSH PROJECT-01 在 Linux 通过，`desktop-e2e-20261031-120850-450`（最终重建树；前次 `desktop-e2e-20261031-115924-542`），Windows/macOS 待补）；S5 步 7 文档/脚本/fixture 引用清理完成（2026-10-31，保留历史记录与 Web `kind: "server"` 路径说明）· 2026-11-01
> 关联：[upstream-v3.16.2-sync.md](upstream-v3.16.2-sync.md)、[rust-unified-host.md](rust-unified-host.md)、[rust-u2-window-host.md](rust-u2-window-host.md)、[rust-u2-host-skeleton.md](rust-u2-host-skeleton.md)
> 上游参考：`shared/src/source-server/*`、`desktop/src/host/sourceServer*`、`server/src/source-server/*`、`docs/architecture/r3-source-server-remote-architecture.md`

## 1. 决策

2026-10-28 用户确认（上游 v3.16.2 同步 Q3）：

1. **本轮（v3.16.2 同步）**只跟进 Source Server 的协议、兼容字段与只读展示；远程 workspace 继续由每窗口 Node Host 承载，Rust Host 不承载远程 scope。
2. **后续阶段**把远程 workspace 统一到 Source Server，并落到 Rust Host；目标状态下 Rust Host 是远程连接与实例表的唯一 owner，最终删除 per-window Node Host 远程路径。
3. 迁移不是“加超时/加兜底”，而是所有权转移：同一时刻只能有一个 Host 服务同一个远程 workspace。
4. 不恢复已删除的手机远控、会话分享、Bots 等产品面；只保留协议兼容字段和桌面端 `desktop-continuous` 语义。

> 2026-10-31 更新：决策 1 只描述 v3.16.2 同步当轮；S5-a/S5-b 已执行决策 2 的 ssh/wsl/docker 迁移，仅 Web `kind: "server"` 自连保留 Node Host。

## 2. 现状与差距

| 项目           | 现状                                              | 上游 Source Server 目标                                         |
| -------------- | ------------------------------------------------- | --------------------------------------------------------------- |
| 本地 workspace | Rust Host 默认（`ESCODE_RUST_HOST` 未设置即为真） | 不变                                                            |
| 远程 workspace | 每窗口独立 Node Host，懒创建，独立进程表          | Rust Host 内统一 registry + 连接 profile + 实例表               |
| 身份           | `workspaceIdentity` + `remoteSessionId`           | `sourceServerId` / Connection Profile / workspace identity 三层 |
| 连接恢复       | Node Host 的远程路径                              | 原位重连、重启恢复、连接日志与错误契约                          |
| 版本/兼容      | 无统一契约                                        | compatibility manifest、匿名信息 schema、严格输出 schema        |
| 项目模型       | 真实 workspace 路径为主                           | 虚拟项目（`virtualProjects`）+ 远程目录绑定                     |
| 更新           | 分散                                              | Host 解析 release、校验兼容、安装、重启、显式失败               |

上游关键能力（v3.16.2 增量，约 59 个源码文件）：

- `shared/src/source-server/`：兼容清单、错误码、连接日志、release 解析。
- `desktop/src/host/sourceServer*`：连接管理、目录、恢复策略、版本解析、启动重连、远端 workspace 准备。
- `server/src/source-server/`：probe、release 下载、transport runtime、adapter。
- `escode-server-cli`：ingress token、proxy stdio、远端资源发布、更新交接。
- UI：三屏连接向导、连接日志、设备卡、Source Server 管理/迁移报告/更新面板。

## 3. 目标状态与所有者

```
┌─ Electron Main（窗口 / 原生 / 进程拉起与转发，不承载业务状态）──────────────┐
│                                                                            │
│   ┌─ Rust Host（每个 App 一个进程）──────────────────────────────────────┐  │
│   │  Gateway（窗口连接注册表、到达序 lane、字节水位出站队列）            │  │
│   │  SourceServerRegistry（sourceServerId、Connection Profile、状态）    │  │
│   │  InstanceTable（每 workspace 一个 runtime 实例，代次与懒重启）       │  │
│   │  runtime-link（embedded / child 双传输，链路之上不区分传输）          │  │
│   │  Services（sessions/tasks、workspace、settings、providers、remote）  │  │
│   └──────────────────────────────────────────────────────────────────────┘  │
└────────────────────────────────────────────────────────────────────────────┘
        │ WebSocket / RPC（capability 握手）
        ▼
Renderer（TS，不直接调用 Repo / Service / Runtime）
        │
        │ SSH / WSL / Docker / 直连
        ▼
Source Server（远端）：目录、release、鉴权、传输；不承载桌面业务状态
```

状态所有者：

| 状态                                                             | 唯一 owner    |
| ---------------------------------------------------------------- | ------------- |
| `sourceServerId`、Connection Profile、连接状态、展示名、更新偏好 | Rust Host     |
| runtime 实例表、代次、订阅归属、调度                             | Rust Host     |
| 会话事实、输入队列、投递、模型请求                               | runtime       |
| 远端目录、release、鉴权、transport                               | Source Server |
| 窗口、原生操作、进程拉起与转发                                   | Main          |

身份规则沿用现有约定：去重/绑定/缓存/队列/持久化/请求关联统一使用 `workspaceIdentity?.trim() || workspacePath`；远程链路贯穿传递 `workspaceIdentity` 与 `remoteSessionId`；`sourceServerId` 进入 identity 后，业务层不得自行解析 authority，统一走 shared 的解析/构造工具。

## 4. 事件顺序

### 4.1 首次连接与绑定

```
Renderer          Rust Host                 Source Server          Runtime
   | connectSourceServer |                        |                    |
   |------------------->|                        |                    |
   |                    | handshake + compatibility                    |
   |                    |----------------------->|                    |
   |                    |<-----------------------|                    |
   |                    | list directories       |                    |
   |                    |----------------------->|                    |
   |                    |<-----------------------|                    |
   | bind project       |                        |                    |
   |------------------->| create/resume instance |                    |
   |                    |-------------------------------------------->|
   |<-------------------| snapshot / continuous  |                    |
```

- 兼容检查失败 → 明确错误码，不静默回退 Node Host。
- 同一 `sourceServerId` 的绑定走 single-flight，避免并发建连。

### 4.2 断线与原位重连

```
断线 ──► Host 标记 unavailable ──► 历史只读、输入不丢
                                      │
下次 get_client ──► single-flight ──► 重连同一 sourceServerId
                                      │
                              兼容版本变化 ──► 显式错误 / 更新流程
```

- 已接受输入持久化；排队输入返回稳定失败码并允许重发。
- 重连不重跑已完成 hook；手动重试只消费当前 failed attempt。

### 4.3 更新与迁移

```
Host 解析 release ──► 校验 compatibility ──► 安装 ──► 重启 Source Server ──► 重连
                                                     └─ 失败：保留旧版本并显式报错
项目迁移 ──► migration report {migrated, failedPathCount} ──► UI 只读报告
```

- 强制更新确认界面可读取运行中任务清单；旧 Source Server 不支持时明确显示“清单不可用”，不推测。
- 迁移部分失败不隐藏，按失败路径数展示。

## 5. 接口与协议

### 5.1 身份

- `sourceServerId` 作为 `workspaceIdentity` 的稳定前缀（`kind=server`）。
- `remoteHostIdFromWorkspaceIdentity` / `buildRemoteHostWorkspaceIdentity` 是唯一 authority 解析/构造入口。
- 不得在业务代码手写 `host:path` 拼接。

### 5.2 serverRemote schema

- `serverRemoteWorkspaceInfoSchema`（输入，容忍同 protocolVersion 下的新增 optional 字段）。
- `serverRemoteWorkspaceInfoOutputSchema`（严格输出，防止泄漏）。
- `serverRemoteInfoSchema`：`compatibility`、`migration.failedPathCount`、`identity {osUser, hostname}`、`virtualProjects`、`capabilities {desktopContinuous, websocketRpc, virtualProjects, processResourceTelemetry}`。
- `serverRemoteAnonymousInfoSchema`：版本、协议版本、`authRequired`、兼容清单。

### 5.3 V4 协议

- `runtimeInitialization`：`initializing | ready | failed`，携带 `attemptId`；冷历史可读不代表 runtime 可执行。
- `v4/sessionsIndex/reconcile`：按 sessionId 查询持久事实，缺席不是删除。
- `sourceServerId` / `serverDisplayName` 进入会话与项目行；`historyFirst` / `historyReadyMs` 进入时序。
- 保留 `desktop-continuous` 语义；`web-remote-replayable` 仅在协议字段层兼容，不恢复手机远控产品面。

### 5.4 Host RPC / 传输

- Host 暴露 Source Server 管理快照、操作日志、状态、展示名、更新动作。
- 远端传输：SSH / WSL / Docker / 直连；`escode-server-cli` 负责 ingress token、proxy stdio、release 资产。
- 远程与本地共用一份 V4 App 协议；链路之上代码不区分 embedded / child。

## 6. 验收场景（后续阶段）

1. 本地 workspace 行为、性能和默认开关不变。
2. 首次连接：握手、兼容检查、目录列表、项目绑定、runtime 启动。
3. 断线：历史可读、输入不丢；原位重连回到同一会话。
4. 桌面重启：恢复连接、项目与侧栏状态。
5. 兼容不匹配：显式错误码和可执行提示，不回退 Node Host。
6. 更新：运行中任务清单可见；安装失败保留旧版本并报错。
7. 迁移：部分失败按路径数展示，不吞错。
8. 同一 Source Server 被两个窗口/两个 workspace 绑定：实例与订阅归属正确、无重复 runtime。
9. Windows / macOS / Linux 三平台。
10. 最终阶段：删除 per-window Node Host 远程路径后，远程链路仍全绿。
11. 差分：Node Host 远程路径与 Rust Host 在同一 fixture 下产出相同 V4 帧（删除前完成）。

## 7. 分期

| 阶段 | 内容                                                               | 退出条件                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| ---- | ------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| S1   | 协议/兼容字段、只读展示（本轮 v3.16.2 同步 W1/W3）                 | schema 测试与类型检查通过；远程仍由 Node Host 服务                                                                                                                                                                                                                                                                                                                                                                                                              |
| S2   | Rust Host 的 SourceServerRegistry、Connection Profile、状态/展示名 | 本地不受影响；远程只读面可用（2026-10-30 已落地）                                                                                                                                                                                                                                                                                                                                                                                                               |
| S3   | 目录绑定、runtime 实例、continuous 投递                            | 首连/断线/重启恢复 E2E（Linux 已通过；Windows/macOS 待补）                                                                                                                                                                                                                                                                                                                                                                                                      |
| S4   | 更新、迁移、恢复策略                                               | 更新失败显式报错；迁移报告可读（Linux E2E 已通过；Windows/macOS 待补）                                                                                                                                                                                                                                                                                                                                                                                          |
| S5   | 删除 per-window Node Host 远程路径                                 | 三平台 E2E 全绿；无静默回退（S5-a/S5-b 已完成；S5-f-1~f-5 已删除 Node Host 远程 handler/文件/共享协议项，§12.6；零 `escode-host-remote-*` 断言：`kind:"server"` 直连已连续两轮三平台通过（run `41268279642`、`41269283544`），SSH 首连：Linux（job `124960188757`）、macOS（run `41328043234`，runner 本机 sshd + darwin-arm64 远端）、Windows（run `41343996871`，job `124961277623`，WSL2 sshd + linux-x64 远端）均已通过；多窗口/重启恢复仍只在 Linux 覆盖） |

## 8. 非目标

- 不恢复手机 Web `/remote`、会话分享、Bots、off-peak、遥测上报。
- ~~不在本轮把远程迁到 Rust Host~~ → 2026-10-31 S5-a/S5-b 已把 ssh/wsl/docker 迁移到 App 级 Rust Host；仅 Web `kind: "server"` 自连保留 Node Host。
- 不新增上游之外的协议语义，不为同步问题加超时兜底。
- 不改变本地 workspace 的默认 Rust Host 行为。

## 9. S2 设计：Host 侧 SourceServerRegistry（只读面）

> 落地记录：2026-10-30。S1 已随 v3.16.2 同步落地（协议 schema、只读展示、`source-server-management` / `source-server-workspace` 服务契约）；S2 从 Host 侧 registry 开始。

### 9.1 所有权与存储

| 状态                                                                     | S2 owner                         | 备注                                     |
| ------------------------------------------------------------------------ | -------------------------------- | ---------------------------------------- |
| `sourceServerId`、显示名、`preferredProfileId`、安装状态、声明 workspace | Rust Host `SourceServerRegistry` | 从 settings 读取，Host 是唯一解释者      |
| Connection Profile（transport / target / credential key）                | Rust Host `SourceServerRegistry` | 只承载 credential key，secret 在凭证服务 |
| 连接状态、操作日志                                                       | Rust Host `SourceServerRegistry` | S2 只投影 `absent` / `failed`（见 9.3）  |
| workspace 打开历史                                                       | Rust Host `SourceServerRegistry` | 只读投影                                 |
| 远程目录、release、鉴权、transport                                       | Source Server（远端）            | 不变                                     |
| 远程 runtime 实例                                                        | Rust Host（S3 起）               | 远端进程由 Source Server 承载            |

- 持久化沿用 app settings 的 `sourceServers` / `connectionProfiles` / `sourceServerWorkspaceHistory` 三个字段，不新增第二个持久化文件；Rust Host 成为这三个字段的唯一写者。S2 首落地时写命令为 `NotImplemented`，写面已在 §9.8 补齐。
- 现状确认：本 fork 目前没有任何代码写入这三个字段（`source-server-management` 未绑定到任何 Host，UI 走 settings 回退），因此 S2 引入 Host 写者不会产生双写。
- settings 根与 Rust Host 的 `rpcListen` home 同源，不再另传路径。

### 9.2 输出严格性

- 快照按 `sourceServerManagementSnapshotSchema` 输出：未知字段必须丢弃（TS 侧 `.strict()`），缺失的可选字段省略而不是写 `null`。
- `generation` 从 1 开始，只在 registry 状态真正变化时自增；纯读取不递增。

### 9.3 S2 接口面

实现：

- `source-server-management.getSnapshot()` → 快照。
- `source-server-management.getRecentOperationLogs(profileId)` → S2 恒为空数组（日志由 S3 的连接流程产生）。
- 事件 `onDidChangeState` / `onDidChangeDirectory` / `onDidAppendOperationLog` → registry 事件总线的独立订阅流。S2 没有发布者，流不发射；S3 的连接与写入流程是唯一发布者，订阅端语义不变。
- 连接动作（`connectProfile` / `disconnectSource` / `updateSource`，S3/S4）与目录写面（§9.8：`renameSource` / `renameProfile` / `updateProfile` / `setPreferredProfile` / `removeProfile` / `removeSourceServer` / `ensureConversationWorkspace` / `confirmIdentityChange`）以及迁移动作（§11.6）均已接入；截至 2026-10-31 该 channel 没有 `NotImplemented` 成员。

保持 `NotImplemented`（缺口显式可见）：

- `source-server-workspace` 整域（S3/S4，见 §10/§11）。

Renderer 绑定（能力广播，S2 内落地）：

- `Initialize` 帧 body 携带 `{channels: string[]}`（排序去重后的已注册 channel 名）；`ChannelClient` 暴露可选 `hasChannel(name)`。
- `RemoteServiceAccess` 仅在 `hasChannel("source-server-management") === true` 时构造代理。per-window Node Host 远程路径没有该 channel，无条件绑定会让远端窗口命中 `Unknown channel`；旧实现不广播时按不可用处理。
- Node Host 的 `Initialize` 改为在 channel 全部注册后发送（`ChannelServer(..., deferInit)` + `ready()`），否则能力清单恒为空。
- 连接时序：能力只在 `Initialize` 之后可知，所以 `connectViaProtocol` 与 `createMessagePortServiceConnection().ready` 都等到握手完成再构建 `RemoteServiceAccess`；`dispose()` 仍可在握手前调用，此时 `ready` 立刻 resolve 并按“能力未知”处理（可选服务不绑定），与原同步构造行为一致。
- 真实 Host 取证：`packages/desktop/test/rustHostRpc.test.ts` 用真实 `escode-host` 二进制 + 真实 `@escode/client` 验证能力广播、条件绑定、严格 schema 快照与事件订阅。
- MessagePort 时序取证：`packages/client/test/messagePortCapabilities.test.ts` 用内存端口对复现 `ChannelServer(deferInit) → ready()` ↔ `createMessagePortServiceConnection()` 真实路径，覆盖“能力存在才绑定”“无能力不绑定”“握手前 dispose 不挂起”三种时序。

状态投影（S2）：

- `profile.lastConnectError` 存在 → `failed`，`error` 取该字段，`updatedAt = lastConnectedAt ?? 0`。
- 否则 → `absent`，`updatedAt = lastConnectedAt ?? 0`。
- 不推断 `online`：S2 没有连接事实，禁止因为 profile 存在就声称在线。

### 9.4 读取路径与一致性

- `getSnapshot` 只在 registry 锁内复制快照，不做 IO；settings 文件只在 Host 启动装配时读一次（S3 引入变更通知后再做增量）。
- 不新增超时兜底；settings 解析失败按显式错误返回，不静默返回空快照。

### 9.5 验收场景

1. 本地 workspace：`pnpm check:escode-host` / `pnpm test:escode-host` 全绿，本地 Host 行为不变。
2. 空 settings：快照为 `{generation:1, profiles:[], servers:[], workspaceHistory:[], states:[], failedPathCount:0, abandonedPathCount:0, migrationCounts:[]}`。
3. 含 profile + sourceServer 的 settings：快照逐字段等于 settings 内容，未知字段被丢弃，不泄漏 `null`。
4. 带 `lastConnectError` 的 profile → `failed`；其余 → `absent`。
5. 契约对账：`host-proto` 的 IMPLEMENTED 与 `host-rpc` 方法表双向一致，未实现成员由 Gateway 回 `NotImplemented`。
6. `getRecentOperationLogs` 对未知 profileId 返回 `[]`，不报错。
7. 真实 Electron 冒烟：本地窗口在握手完成后拿到 `sourceServerManagementService`；SSH 远端窗口不绑定该服务（`managementBound=false`），且运行日志 `Unknown channel` 为 0。
8. 远端 scope 服务合并：`buildRemoteWorkspaceSessionServices` 的 Source 管理/workspace 服务必须取自远端 access，不得从 `baseServices` 继承；本地管理服务只属于本地 scope。
9. UI 投影等价：同一份 profiles/history 经 `sourceServerManagementSnapshotSchema` 与 `appSettingsSchema` 解析后，`projectRemoteWorkspaceSessionIdentityAliases` 输出逐项相同（含 `generation` 判序与 rekey）。

### 9.6 不在 S2 范围

- 写入（rename / remove / setPreferredProfile / updateProfile）与连接动作。
- 真实连接状态、操作日志来源、continuous 投递、实例表（S3）。
- 更新与迁移（S4）。
- 删除 per-window Node Host 远程路径（S5）。

### 9.7 交付记录与结论

已交付（2026-10-30）：

- `host-services::source_server`：`SourceServerRegistry`（settings 装载、`from_settings`、`snapshot`、`recent_operation_logs`），未知字段在输出侧丢弃，装载失败显式报错；8 个单测。
- `host-rpc::domains_source_server`：注册 `source-server-management`，实现 `getSnapshot` / `getRecentOperationLogs` 与三个事件订阅；其余成员仍由 Gateway 回 `NotImplemented`。
- 能力广播：`Initialize` 携带已注册 channel 名，`ChannelClient.hasChannel` + `RemoteServiceAccess` 条件绑定；Node Host 改为注册完 channel 后 `ready()`。
- 连接时序单测：`channelClient.waitUntilInitialized()` 等握手后再建访问器；`createMessagePortServiceConnection` 保持同步返回 + `ready` promise，electron-free 集成测试覆盖 MessagePort ↔ ChannelServer 能力绑定与握手前释放。
- 契约对账：`host-proto` 新增 `IMPLEMENTED_SOURCE_SERVER`（拆文件守住 400 行上限），`tests/contract.rs` 双向对账通过。
- `pnpm check:escode-host`（fmt / clippy `-D warnings` / 服务清单 / 边界）通过。
- UI 投影等价单测：`packages/ui/test/remoteWorkspaceSessionPersistence.test.ts` 用生产 schema 分别解析同一 profiles/history，断言两种来源的 alias 投影逐项相同。
- 远端 scope 缺陷修复：`packages/desktop/src/renderer/src/remoteWorkspaceSessionServices.ts` 显式覆盖 `sourceServerManagementService` / `sourceServerWorkspaceService`，避免远端 session 继承本机 Source 管理服务；回归单测 `packages/desktop/test/remoteWorkspaceSessionServices.test.ts`。
- Renderer 启动与 Node Host 远端窗口回归 E2E：`packages/desktop/test/e2e/source-server-capability.test.ts` 覆盖本地窗口握手后绑定，以及真实 SSH 远端窗口（设备选择 → 目录选择 → workspace 接管）不绑定该服务；artifact `desktop-e2e-20261030-140615-815`，2 passed，运行日志 `Unknown channel` 0 次。
- 远端 E2E 需要 `ESCODE_E2E_SSH_REMOTE_ASSET_PLATFORMS=linux-x64`，否则 remote assets 准备阶段会误构建 aarch64-musl；无 SSH 环境时用例 skip。

S2 结论（2026-10-30）：Host 侧 registry 只读面、能力广播、客户端条件绑定、Renderer 启动时序与远端窗口回归均已验证。远程 workspace 仍由 per-window Node Host 承载，S3 从 §10 开始。

### 9.8 S2 写面：settings 唯一写者（2026-10-31 落地）

设置面板、远端连接向导与远端会话入口已经按 `ISourceServerManagementService` 调用这些写方法；Rust Host 不能继续回 `NotImplemented`，否则切到 Rust Host 后本地设置页会整块失效。

总则：

- 唯一写入口是 `SourceServerRegistry`（`host-services::source_server_writes`）。每次写先取 registry 写锁，再经 `settings::apply_patch` 落盘（与 `setting.update` 共用同一 `settings_lock`）；只有落盘成功后才更新内存快照、递增 `generation` 并发布 `onDidChangeDirectory`。失败不改内存、不递增 generation。
- 只写 `sourceServers` / `connectionProfiles` / `sourceServerWorkspaceHistory` / `remoteConversationWorkspaceBySourceServerId` 四个顶层键；其它键（含 TS legacy 字段）原样保留。
- 输出仍是 `sourceServerManagementSnapshotSchema` 的严格形状：写回时按 Rust 结构重序列化，未知字段不进入 settings。
- 幂等空操作不写盘：`removeProfile` 的未知 profile、`removeSourceServer` 没有任何本地记录（含 conversation workspace）时直接返回当前快照。
- 校验失败显式报错、不写盘：空显示名、未知 Source/Profile、Profile 不属于该 Source、Profile 已 retired、transport 不可原地变更、SSH 认证方式缺失/非法。
- 凭据与 settings 同处一个回滚边界：`updateProfile` 的凭据轮换、`removeProfile` / `removeSourceServer` 的凭据删除，任一步失败都恢复此前凭据值；缺 `dataBaseDir` 时 SSH 凭据操作显式失败，不回退成明文写入。

逐方法语义：

| 方法                                       | 行为                                                                                                                                                                                                                                                                                                                                                                                                                          |
| ------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `renameSource(id, name)`                   | trim 后非空；更新 `displayName`；返回新快照。                                                                                                                                                                                                                                                                                                                                                                                 |
| `renameProfile(id, name)`                  | trim 后非空；更新 `displayName`；返回新快照。                                                                                                                                                                                                                                                                                                                                                                                 |
| `updateProfile(input)`                     | 原位更新 `displayName` + `target`（保持 `profileId` / `sourceServerId` / history 稳定）。transport 必须与 target.kind 一致且不可改。SSH 按 `sshAuthMethod` 轮换：密码复用/生成 `connection-profile:<profileId>:password`，私钥口令复用/生成 `connection-profile:<profileId>:private-key-passphrase`，切换时删除另一条凭据。可选的 `sourceDisplayName` 同步设备名（要求 Profile 已绑定 Source）。成功后清 `lastConnectError`。 |
| `setPreferredProfile(sourceId, profileId)` | 只接受属于该 Source 且 `lifecycleState !== "retired"` 的 Profile；写 `preferredProfileId`。                                                                                                                                                                                                                                                                                                                                   |
| `removeProfile(profileId)`                 | 先由连接管理器收口该 Profile，再删除 Profile、其 workspace history，并把以它为 `preferredProfileId` 的 Source 改指向同 Source 的第一个 active Profile（没有则省略该键）；删除 SSH 凭据。                                                                                                                                                                                                                                      |
| `removeSourceServer(sourceId)`             | 先 `disconnectSource`，再删 Source、其全部 Profile、按 `sourceServerId` 或被删 `profileId` 命中的 history、`remoteConversationWorkspaceBySourceServerId[sourceId]`；删除全部 SSH 凭据。没有任何本地记录时幂等成功，不产生写入。                                                                                                                                                                                               |
| `ensureConversationWorkspace(sourceId)`    | 必须有该 Source 的在线连接，否则报 `Source Server must be connected: <id>`；经远端 `project.prepareSession` 分配目录；把 `{workspacePath, lastResolvedAt}` 写入 `remoteConversationWorkspaceBySourceServerId`，并清掉同 Source 同路径的 project history；返回 `{path, created:true, workspacePurpose:"conversation"}`。                                                                                                       |
| `confirmIdentityChange(profileId)`         | 按用户确认重连该 Profile：连接流程以 `confirmIdentityChange=true` 记录握手；若远端 `sourceServerId` 变化，生成新 Profile id、复制/重写 SSH 凭据、retire 旧 Profile 并保留旧 history，成功后返回管理快照。未确认的连接返回 `source-server-identity-change-pending` 且零写入。见 §11.12。                                                                                                                                       |

`updateProfile` 的目标归一化（写入 settings 的严格 schema）：

- SSH：`host` trim + 小写、`port` 缺省 22（整数 1..=65535）、`username` trim 非空；保留 `sshConfigAlias` / `privateKeyPath` / 合法的 `assetInstallMode`；`password` / `privateKeyPassphrase` 只作输入，永不落盘。`resourcePackages` 仅在本分支 schema 校验通过时保留（当前为 no-op，未知 id 丢弃整键）。
- WSL：`distro` / `user` trim；`user` 必须满足 shared `isValidWslUser`（≤64、无控制字符、不含 `:` `/` `\`）。
- Docker：`container` trim 非空。

验收场景：

1. 设置页重命名设备/连接后，快照与 `setting.json` 同步更新，`generation` 递增一次，其它设置键不变。
2. `removeProfile` 后 Profile 的 SSH 凭据不可再读取；作为 preferred 的连接被删后自动指向同设备另一个 active 连接。
3. `removeSourceServer` 清掉该设备的 Profile、history 与 conversation workspace；对不存在的设备重复调用不写盘。
4. `updateProfile` 密码 ↔ 私钥切换后只保留新认证链的凭据；settings 中不含明文口令；任一步失败时旧凭据仍可读取。
5. `setPreferredProfile` 拒绝跨设备或 retired Profile。
6. `ensureConversationWorkspace` 在未连接时显式失败；在线时返回远端真实 cwd，并写入设备级 conversation workspace，不污染 strict project history。

## 10. S3 设计：目录绑定、runtime 实例与 continuous 投递

> 落地记录：2026-10-30 开始。S3 的目标是让 Rust Host 成为远程 workspace 的连接与实例 owner，Renderer 不再为远程 workspace 创建 per-window Node Host；Node Host 远程路径保留到 S5 删除，期间不得出现双 Host 同时服务同一 workspace。

### 10.1 所有权

| 状态                                              | S3 owner                         | 备注                                    |
| ------------------------------------------------- | -------------------------------- | --------------------------------------- |
| transport 选择、连接尝试、连接状态、显式错误      | Rust Host                        | profile 只描述目标，不描述运行时状态    |
| `sourceServerId` ↔ `workspaceIdentity` ↔ 远端目录 | Rust Host `SourceServerRegistry` | 绑定是唯一事实；Renderer 不自行拼接身份 |
| runtime 实例、代次、订阅归属、懒重启              | Rust Host `Instances`            | 与本地 workspace 共用同一张实例表       |
| 远端目录内容、release、鉴权、transport 物理链路   | Source Server（远端）            | 不变                                    |
| 窗口 attachment / optimistic overlay              | Renderer                         | 只持有未提交草稿，不作为连接事实        |

### 10.2 实例表按身份选择链路（已落地切片）

`Instances` 原先只持有一条固定 `Link`，只能服务同一类 workspace。S3 起实例表按 workspace identity 选择链路：

```text
Instances::get(identity)
  └─ link_for(identity) ──► Link（embedded / child / 后续 remote）
       └─ RuntimeLink::open(link, options) ──► Instance(gen N)
```

- `Instances::new(link)` 保持不变，等价于所有身份使用同一条链路（本地 workspace 的现有行为）。
- 新增 `Instances::with_link_for(|identity| -> Link)`：组合根按身份注入链路；同一身份仍走原槽位单飞与懒重启。
- 代次、single-flight、崩溃回收、`shutdown` 残余清单语义不变；链路选择只发生在真正启动的路径上，复用活实例不调用选择器。
- 选择器返回的链路启动失败时，不占用代次（回退到上一代），错误显式返回给调用方；不影响其他身份。
- 链路形态：`Embedded`（本地内嵌）、`Child`（本地 CLI 子进程）、`Process(ProcessSpec)`（任意 program/args/env，供 ssh/wsl/docker transport 直接使用）；三者对 `Instance` 同形。

### 10.3 传输与帧面

现状核查（2026-10-30）：

- 本地 runtime 链路（`Embedded` / `Child` / `Process`）的帧面是 `app-server --stdio` 的 NDJSON；`Instance` 的读帧、路由与事件投影都建立在这个帧面上。
- 现有远端路径不是 NDJSON：`packages/server/src/remote/connect.ts` 连接 `escode-server-cli` 后先做 `escode-hello` / `escode-hello-ack` 握手，再用 `SocketProtocol` 的 VSBuffer 二进制帧（13 字节头 + payload）暴露整套服务。Rust Host 目前没有该协议的客户端实现。（2026-11-01：该 Node connect/deploy 路径已随 S5-d 删除，协议事实保留作 Rust 客户端依据。）
- 因此 `Link::Process` 只能承载 NDJSON runtime，不能直接承载现有远端 server；远端 workspace 的接入必须先选定下列之一，且不得在同一链路上混用两种协议：
  - A. 远端运行与 Rust Host 同协议的组件（或兼容 NDJSON 的 runtime），Rust Host 继续用 `Link::Process`；远端 host 服务边界需另行定义。
  - B. Rust Host 实现 VSBuffer RPC 客户端与 scoped 转发，兼容现有 `escode-server-cli` / Node server；远端服务仍由 Source Server 提供。

2026-10-30 选定 B：现有 Source Server 已经是帧面协议，A 需要重写远端组件。B 的客户端底座已落地（§10.9），后续实现 scoped 转发。

无论选型如何：

- transport adapter 只负责把 Connection Profile 解析成一条可读写的链路；SSH/WSL/Docker/直连差异不得进入业务域。
- 连接、鉴权、兼容性与安装失败必须以显式错误码返回并停止本次尝试；S5 之前，未被 Rust 接管的 workspace 仍可由 Node Host 服务，但同一 workspace 不允许两条路径同时在线。
- 选型与据此新增的 crate / 模块在实现前补进本节；不先写“看起来通用”的空转发层。

### 10.4 目录绑定

- `source-server-workspace.resolveWorkspace(workspaceIdentity)` 在 S3 返回已绑定的 `{sourceServerId, profileId, remotePath}`；未绑定时返回显式 `unbound`，不按路径猜测。
- 绑定写入只由 `ensureConversationWorkspace` / 目录选择流程产生；同一 `(sourceServerId, remotePath)` 无论多少窗口，只对应一个实例身份。
- 绑定变化发布 `onDidChangeDirectory`，并递增 registry `generation`；订阅方以 generation 判断是否需要重新解析。

### 10.5 连接状态机

```text
idle ──connect──► connecting ──transport ok──► handshaking ──protocol ok──► ready
                     │                              │                        │
                     └── error (code) ◄─────────────┴── error (code) ◄────────┘
ready ──transport lost──► disconnected ──retry──► connecting
ready ──user disconnect──► idle（显式收口，不自动重连）
```

- 每次尝试携带 `attemptId`；过期的成功/失败结果不得覆盖新一代状态。
- 连接 `ready` 不等于 runtime `ready`：`runtimeInitialization`（`initializing | ready | failed`）单独投影，冷历史可读不代表可执行。
- 断线保留可读历史；重连回同一 logical session，不按时间接近推断身份。

### 10.6 continuous 投递

- 每个实例只有一条上游帧流；窗口 attachment 以 `(workspaceIdentity, generation)` 订阅，实例换代后旧订阅显式结束，新订阅从新一代读取。
- 同一 workspace 的多窗口共享实例与上游，各自维护投递游标；实例不因窗口关闭而停止，窗口关闭只移除订阅。
- `desktop-continuous` 语义保持：断线期间历史可读、输入不丢，已接受的 busy/running 输入仍由 CLI/runtime `CommandInbox` 串行 admission。

### 10.7 重启恢复

- App 重启只从 settings 的 `sourceServers` / `connectionProfiles` / `sourceServerWorkspaceHistory` 恢复绑定，不把内存状态当事实。
- 实例表不持久化进程句柄；重启后按需懒启动新代次，旧代次只作为记账与日志。
- 自动重连沿用现有产品规则（近 7 天有 `lastOnlineAt` 的 Source / 已打开的断连 tab），不扩大为“所有历史 workspace 自动重连”。
- S3 只保证重启后恢复断连 tab 与绑定；Host 启动自动连接 profile（`lastOnlineAt` 7 天门禁）仍属 S4。手动重连必须回到 Rust Host，不得因 target 快照缺少 Profile 归属而回退 per-window Node Host。

### 10.8 验收场景（S3）

1. 首连：SSH fixture → 选择目录 → 绑定 → runtime ready → V4 帧连续到达窗口。
2. 断线：终止 transport → profile `offline`、tab 进入断连占位 → 原位重连 → 同一逻辑会话（task/历史）可续，Host `remoteSessionId` 允许换代。
3. App 重启：重启后恢复绑定与断连 tab，不重复启动同一 workspace 的实例。
4. 多窗口/多 workspace：同一远端目录只起一个实例；不同目录各自实例与代次。
5. 显式失败：不可达/鉴权/兼容失败返回错误码，不静默回退 Node Host。
6. 实例表：按身份选择链路、同身份复用、启动失败不占代次（已由单测覆盖）。

### 10.9 S3 进度

- 已落地（2026-10-30）：`Instances::with_link_for` 与按身份选择链路的单测；`Link::Process(ProcessSpec)` 与“任意进程链路帧面与 `Child` 一致”的单测；`Instances::new` / `Link::Child` 行为不变。
- 已落地（2026-10-30）：`host-services::source_server_transport` 把 Connection Profile 解析成 ssh / wsl / docker 启动命令（`TransportCommand`），远端命令统一经 POSIX 单引号转义；密码/口令缺失显式报错，secret 只进子进程 env，不进 argv；6 个单测含与 TS `quotePosixShellArg` / `quotePosixPathArg` / `resolvePosixHomePath` 的逐字节对齐。
- 已落地（2026-10-30）：Host 二进制 `--ssh-askpass` 模式，只输出 `ESCODE_SSH_ASKPASS_SECRET` 后退出；2 个进程级测试。
- 已落地（2026-10-31）：askpass 识别覆盖 OpenSSH 的真实调用形式——`SSH_ASKPASS` 子进程只把提示语放在 `argv[1]`（如 `root@host's password:`），不会附带 `--ssh-askpass`。Host 正常启动不带位置参数，因此“存在位置参数”即进入 askpass 模式；否则 Host 会启动控制通道、把 `hostReady` 帧当作密码写给 ssh（S3 E2E `handshake timeout: no escode-hello` 的根因）。进程级测试覆盖提示语参数、显式 flag 与缺失 secret 的显式失败。
- 已落地（2026-10-31）：远端 stdio `ChannelServer` 改为 defer-init，等 `exposeOnChannelServer` 注册完全部 channel 后再 `ready()` 广播能力。此前构造期立即发 Initialize，广播里的 `channels` 恒为空；Rust Host 按广播决定转发入口（§10.11），导致远端 scope 只剩 `window-controller`，`file.readdir` 报 `Host does not implement channel \`file\` yet`（S3 E2E 目录选择失败根因）。回归测试用队列协议断言 Initialize 列出已注册 channel。
- 已落地（2026-10-31，S3 真实 Rust E2E）：`packages/desktop/test/e2e/conversation-session/source-server-rust-remote-scope.test.ts` 先种子 `sourceServers` + SSH `connectionProfiles` + credential，经侧栏“项目 + → 远程连接”选择已保存设备，由 Rust Host 打开 `/root/workspace` 与 `/root/workspace-b`。断言 profile 状态 `absent → online`、两个 workspace identity 正确、`remoteSessionId` 不同、进程表无 `escode-host-remote-*`。绿跑 `desktop-e2e-20261030-183727-199`：`PASSED` / `1 passing (10.5s)`；日志含 4 条 `remote scope capability issued`（同一 profileId/sourceServerId）与 `renderer attachment ready`（connect + workspace-context-bound 各两代）。该用例只证明 Connection Profile → Rust Host 路由；首次直连 target 无 `profileId`，S5 前仍走 per-window Node Host。
- 已落地（2026-10-30）：`host-core::rpc_wire`（13 字节 Regular 帧编解码、分片/粘包、非 Regular 跳过、256 MiB 上限）与 `host-core::rpc_serialization`（Undefined/String/Buffer/VSBuffer/Array/Object/Int + VQL），与 TS `protocol.ts` / `serialization.ts` 逐字节对齐；11 个单测。
- 已落地（2026-10-30）：`host-services::source_server_handshake`（banner 跳过、hello 校验、ack 字段顺序）与 `host-services::source_server_client`（Initialize 等待与能力广播、Promise/事件/取消、错误 code/name 透传、dispose fail-closed）；5 + 5 个单测。
- 已落地（2026-10-30）：跨语言对拍 `apps/escode-host/tests/source_server_client_ts.rs`，Rust 客户端直连真实 TS `ChannelServer`，覆盖 Initialize、参数往返、100 KB 载荷、事件、PromiseError code；fixture `apps/escode-host/tests/fixtures/rpc_server_fixture.mjs`。
- 已落地（2026-10-30）：`host-services::source_server_connection` 串起整条 stdio 栈：启动 `TransportCommand` → 读 `escode-hello`（超时 15s，EOF/超时显式报错）→ 回 ack → 帧面泵（stdout 解码、stdin 写帧、stderr 转 warn）→ `RemoteChannelClient`；`shutdown` 先 fail-closed 拒绝挂起请求再终止子进程。
- 已落地（2026-10-30）：同一对拍文件的 `connects_over_stdio_with_handshake` 走完整 `connect()`，覆盖 hello 解析、ack 往返、握手后的帧面（Initialize → `getSnapshot` 往返 → `shutdown`），以及无 hello 时的显式超时。fixture 在 hello 模式下与 `packages/server/src/entry-stdio.ts` 一致地先消费 ack 行再启动 RPC（ack 字节不得进入帧面）。
- 已落地（2026-10-30，S3-a）：`host-services::source_server_connections` 连接管理器（§10.10）——per-profile 单飞与尝试代次、`bootstrapping → online | failed`、断链 `offline`、显式收口 `absent`；状态只写 registry（`set_connection_state` 递增 generation 并广播 `onDidChangeState`），不另存副本。
- 已落地（2026-10-30，S3-a）：`source_server_connection::RemoteConnection::closed()` 暴露链路结束信号（reader/writer 退出或显式 shutdown），管理器的 watcher 以代次判序把过期关闭结果丢弃。
- 已落地（2026-10-30，S3-a）：`source-server-management.connectProfile` / `disconnectSource` 与组合根装配（profile → ssh/wsl/docker 命令，密码/口令经 `credential` 域解密后只进子进程 env）；远端命令默认值与 TS `buildRemoteServerCommand` 对齐，deploy/preflight 留到 S4。
- 已落地（2026-10-30，S3-a）：测试——连接管理器单测 4 个（未知 profile、transport 解析失败、收口幂等、运行态覆盖 settings 投影）；RPC 面 `crates/host-rpc/tests/rpc_source_server.rs` 2 个；跨语言 `source_server_client_ts.rs` 新增 `connection_manager_reaches_online_and_disconnects` 与 `link_loss_marks_offline`（真实 TS fixture，覆盖 online → 调用 → 断链 offline → 收口 absent）。
- 已落地（2026-10-30，S3-b）：`POST /api/rpc-host-capability` 接受 `{"scope": {...}}`：先经 S3-a 连接管理器建立/复用 profile 连接（`Initialize` 完成）再签发绑定客户端句柄的一次性 capability；失败回 400 + `error`，不签发任何令牌（含本地权限）。HTTP 读取改为「头 + body」（`http::read_request`，body ≤ 64 KiB），避免 `read_head` 连带消费 body。
- 已落地（2026-10-30，S3-b）：`host-rpc::remote_scope` 转发面——远端 scope 连接的 channel 集合 = 显式允许表（`REMOTE_SCOPE_HOST_CHANNELS`，当前 `window-controller`）+ 远端 `Initialize` 广播且存在于 `host-proto::TS_SERVICE_CHANNELS` 的转发入口；方法走 `call`、`on` + 大写成员走 `listen`，流丢弃时 `dispose_listener`，远端错误保留 `name`/`message` 并透传 `data.code`。
- 已落地（2026-10-30，S3-b）：测试——`remote_scope` 单测 5 个（方法 Promise 帧、事件订阅与销毁帧、错误 code、未广播 channel 不转发、`Wire↔WireValue` 含 100 KB 字节串）、`capability` 单测 2 个、RPC 面 `crates/host-rpc/tests/rpc_remote_scope.rs` 3 个（允许表隔离、本地行为不变、签发失败不签发）、跨语言 `apps/escode-host/tests/source_server_remote_scope_ts.rs` 1 个（真实 TS fixture 经 `serve()` + scoped capability 完成调用与事件）。
- 已落地（2026-10-30，S3-c-1）：`RemoteTarget` 的 ssh/wsl/docker 新增可选 `sourceServerId` / `profileId`（shared 类型 + 运行时校验），`buildRemoteTargetFromProfile` 从 Connection Profile 填充；未携带 `profileId` 的直连 target 仍走 Node Host。
- 已落地（2026-10-30，S3-c-1）：Main 远端路由按 `RemoteAttachmentRoute.owner` 分流（§10.12）——`issueRustHostCapability` 发送 `{"scope"}` body，`rustHostManager.attachRemoteScopePort` 桥接 RPC WebSocket（S3-c-3b 后桌面远端改用 `desktop-continuous` → `/ws/host`）；rust route 跳过 Node Host 的 connect/bind/provider/dispose 消息，bind 后按新 identity 重签并换代 attachment，晚到候选与旧句柄在 ready 时释放。
- 已落地（2026-10-30，S3-c-1）：测试——`packages/desktop/test/rustHostRemoteScope.test.ts` 7 个（scope body、Host 4xx 错误透传、`/ws` 双向帧桥接、create/attach 不触碰 Node Host、bind 换代释放旧句柄、失败不留 route、Host 不支持远端 scope 时 `REMOTE_RUST_HOST_UNAVAILABLE`）；`packages/ui/test/useRemoteWorkspaceHistory.test.ts` 覆盖 profile 字段随 target 传递。
- 已落地（2026-10-31，S3-c-2a）：远端 scope 的连接级 `window-controller`（§10.13）——`RemoteController` 以 `escode-task` 为唯一事实源，`listTasks`/`listPinnedTasks`/`listArchivedTasks` 合并 membership，`onDynamicWorkspaceEvent` 置脏后 single-flight 刷新；`listTaskList` 搜索收敛到本连接 scope，非搜索只读投影；`mutateTask` / `deleteArchivedTask(s)` 转调远端写面，刷新失败保留旧投影并 warn；行地址/workspace fact 带 `sourceServerId` + stable `workspaceIdentity`，不使用 `remoteSessionId`。`Channels::connect` 对远端连接按 channel 名去重并保留连接级实现。
- 已落地（2026-10-31，S3-c-2a）：测试——`window_controller::remote` 单测 8 个（列表与行地址、pin/archive/mark-read 写面与 membership、跨 scope 拒绝、缺失 `escode-task` 显式错误、搜索 scope 收敛与叠加、搜索失败降级为空、不可解析 identity 拒绝写入、本地/远端键隔离）；跨语言 `source_server_remote_scope_ts.rs` 新增 2 个（fixture `escode-task` 的订阅快照 + `mutateTask` + resync + 跨 scope 拒绝，以及未注册 `escode-task` 的显式错误）。
- 已落地（2026-10-31，S3-c-2b 调研结论）：`file-watcher` 与 `media-preview` 不需要 Host 本地合流——远端 scope 已把它们作为远端广播 channel 转发到 Source Server（§10.11），远端窗口拿到的是远端文件面；`media-preview` 继续走 Source Server 的 inline（≤ 8 MiB）分支，超限显式失败。`source-server-workspace.resolveWorkspace` 的 canonical alias/迁移是 Source Server Core 的权威 seam（R3 架构 §3.2），未接入 Core 前 Rust Host 不实现、也不按路径猜测。
- 已落地（2026-10-31，S3-c-3a）：远端链路断开传导（§10.14）——`RemoteChannelClient::wait_closed` 观察 EOF/`dispose`；绑定 grant 的 scope RPC 连接在链路终结时结束，不再留下失效 `/ws`。`bridgeRustHostPort` 只在非主动关闭时上报，Main 将 rust route 标 `closed` 并发送既有 `RemoteSessionClosed`；Host 退出仍走 `onLost`（`host-exited`），socket 关闭走 `onClosed`，两者分开避免进程退出时序把原因误判。同时修复写任务持有出站 `tx` 导致 Host 主动关闭时 `writer.await` 永不结束的问题。
- 已落地（2026-10-31，S3-c-3a）：测试——`rpc_remote_scope` 新增「链路关闭只收口 scope 连接」；`source_server_client_tests` 新增 `wait_closed`；`rustHostRemoteScope` 新增非主动关闭回调与 route 收口通知；`rustHostResilience` 保持 Host 退出 `host-exited` 断言。
- 已落地（2026-10-31，S3-c-3a）：验证——`cargo test -p escode-host-rpc --test rpc_remote_scope` 4 passed、`--lib` 180 passed、`wait_closed_observes_disposal` 1 passed；`pnpm exec vitest run packages/desktop/test/rustHostRemoteScope.test.ts packages/desktop/test/rustHostResilience.test.ts` 21 passed；`pnpm typecheck` / `pnpm lint` / `pnpm architecture:check --changed` / `pnpm check:escode-host` 通过；`pnpm test:escode-host` 全量 Rust workspace 与 Node/Rust settings interop 2 passed。
- 已落地（2026-10-31，S3-c-3b）：桌面远端 attachment 改走 `desktop-continuous`（`/ws/host`），手机/Web 共享 Host attachment 保持 replayable（§10.15）；V4 `escode-agent` 仍全转发到 Source Server，投递差异由 Source Server 权威决定，Host 不改写。
- 已落地（2026-10-31，S3-c-3b）：测试——`rustHostRemoteScope` 新增桌面远端 `/ws/host` 路径断言；跨语言 `source_server_remote_scope_ts` 的 `v4_delivery_facts_are_forwarded_without_host_rewriting` 覆盖 continuous/replayable 两种 fixture 配置（hello 模式、订阅快照/增量帧、命令 clientId/inputOrigin 原样转发、两种模式下 `window-controller.listTaskList` 读取同一远端事实）；`pnpm typecheck` / `pnpm lint` / `pnpm architecture:check --changed` / `pnpm check:escode-host` 通过，`pnpm test:escode-host` 全量 Rust workspace 与 interop 2 passed。
- 已落地（2026-10-31，S3-c-3c）：断线原位重连验证——rust route 收到 attachment close 后只收口旧 route，同一窗口/同一 target/workspaceIdentity 可以新 requestId 重新申请 capability 并 attach；renderer 既有 reconnect 流程复用断连 tab、替换 `remoteSessionId`，Node Host 全程零调用。测试：`rustHostRemoteScope` 新增 close → reconnect 用例（新 sessionId、同一 scope、旧 attachment dispose 一次）。
- 已落地（2026-10-31，S3-c-3d）：Node/Rust 远端 V4 帧差分（§10.16）——`fixtures/v4_node_oracle.mjs` 用真实 `ChannelClient` 连 fixture，再经等价 per-window Node Host 转发，输出 hello、订阅 ack、按 `kind` 排序的事件帧与命令 ack 的规范化 JSON；跨语言 `source_server_remote_scope_ts::v4_frames_match_the_node_remote_oracle` 在 continuous（`/ws/host`）与 replayable（`/ws`）两种配置下逐字段对拍，忽略 `serverTime`/`connectionId`。
- 已落地（2026-10-31，S3-Linux E2E）：真实 SSH fixture（独立 sshd 容器 + 已部署 Source Server）跑 formal `packages/desktop/test/e2e/source-server-capability.test.ts`：2 passed。远端窗口获得 `remoteSessionId`，`baseBound=true`、`managementBound=false`；日志显示远端 `escode-agent` 由容器内 Source Server 启动，Rust Host 与 Source Server 进程分离，per-window Node Host 未服务该远端 workspace。本轮只在 Linux x64 执行；Windows/macOS 待补（多窗口/App 重启见下一项）。
- 已落地（2026-10-31，S3 恢复/多窗口 E2E）：远端 workspace 绑定后统一使用 `remote:server:<sourceServerId>:<workspacePath>` 身份，Profile 归属（`profileId` / `sourceServerId`）随远端 target 快照与重连历史持久化，避免重启或多窗口因 `remote:ssh:*` 身份被判定为非 Source 归属而跳过 Rust Host。单测：`rustHostRemoteScope`、`remoteWorkspaceHistory`、`useRemoteWorkspaceHistory` 共 110 passed。真实 SSH formal E2E（Linux x64，`desktop-e2e-20261030-192803-622` 与重构后复跑 `desktop-e2e-20261030-193520-064` 均 3 passed）：首连双 workspace；App 重启恢复断连 tab 后手动重连回到 Rust Host（profile `absent` → `online`，无 per-window Node Host）；同一 workspace 双窗口共享单一 `ESCode agent process`、各持独立 attachment（4× capability、2 个不同 `remoteSessionId`、4× renderer attachment ready）。
- 已落地（2026-10-31，S3 真实链路断开）：新增 `source-server-rust-transport-drop.test.ts`——经 Connection Profile 首连并完成一轮对话后，E2E 在本地进程树中定位 Rust Host 的 `ssh` transport 子进程并 `SIGKILL`，断言 profile 收敛 `offline`、tab 清空 `remoteSessionId`，再点侧栏重连回到 Rust Host（新 `remoteSessionId`、`online`、单 tab、无 per-window Node Host），并续发一轮验证断线前历史与同一 task 仍可用。复跑 `desktop-e2e-20261030-201548-326` 1 passed（19.8s）；四例合跑 `desktop-e2e-20261030-201824-159` 4 passed（1m54s），确认与首连/重启/多窗口用例无相互干扰。已知边界：Rust Host 尚未 provisioning 本地 Provider 到远端（S4），该用例经 SSH 写入并在 teardown 清理一个指向 reverse tunnel 的远端 replay Provider；Windows/macOS 无法在当前环境验证。
- 已落地（2026-10-31，S4-a）：远端 `escode-hello` 增加可选 `compatibility`（`compatibilityManifestSchema.strip()`），TS `helloMessageSchema` / `HelloMessage` 容忍旧 server 缺失该字段；Rust `source_server_compatibility` 严格解析清单并按 TS 规则比较版本（含 prerelease），连接管理器在兼容失败时写 `needs-upgrade + source-server-upgrade-required + upgradeTarget=server` 或 `failed + upgradeTarget=controller`，兼容时写 `updateAvailability` 与 `minimumVersion`（有更新时附 `availableVersion`）。`ConnectionStateSnapshot` 补齐四个更新字段（缺失省略，不写 `null`）；registry 新增 operation log 保留与广播，连接流程写 `connecting/online/failed` 日志。验证：`cargo test -p escode-host-services` 589 passed；`packages/shared/test` 1351 passed；`pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed`、`cargo clippy -D warnings` 通过。
- 已落地（2026-10-31，S4-b）：`source_server_release` 严格解析 component manifest（schemaVersion/appVersion/platformArch、id 规则与去重、sha256、mount 白名单、相对路径安全、未知组件跳过），`component_cache_version` 只剥离 `+<12..64 hex>`；`should_deploy_component` / `check_component_identity` 与 TS `shouldDeployVersionedComponent` / `checkRemoteAssetComponentIdentity` 的判定顺序一致，`ComponentMeta` 序列化字段顺序与 Node 相同，`InstallPreview` 由 appVersion + `$HOME/.escode/server` + 安装包字节数组成，并已挂到 `ConnectionStateSnapshot.installPreview`（缺失省略）。`source_server_preflight` 用一次 POSIX shell 往返探测 uname/`/proc`/`HOME` 与 `curl|wget`、`tar`、`sha256sum|shasum|openssl`，平台/架构归一化对齐 `detectEnv.ts`。验证：`cargo test -p escode-host-services` 604 passed（新增 15 个单测）；真实 Docker SSH fixture（127.0.0.1:22224）preflight 返回 `linux-x64` / `Curl` / `Sha256sum` / `/root`；`pnpm check:escode-host`、`pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed` 通过。
- 已落地（2026-10-31，S4-c）：
  - `source_server_bootstrap` 编排首次安装：一次 preflight 探平台/工具，读 `releases/<version>/manifest-<platformArch>.json`，一次脚本探测远端组件存在性，只安装缺失组件（版本不同的已存在组件不在连接流程替换，留给 §11.4 显式更新）；全部就位返回 `None`，本地无 release 且 `node` + `zcode-server.cjs` 已存在时继续连接（保留 S3 行为）。
  - 两种来源：本地产物齐全走 `local-upload`（归档复用 release 目录树）；否则走 `remote-download` 且发布制品以 component `mount` 为归档根（与 TS `buildRemoteComponentSourcePath` 同义）。`install preview` 只报告可本地计算的大小（本地上传=归档字节；远端下载=本地 artifact 大小），不可计算时省略。
  - `source_server_deploy_lock` 与 Node `remoteDeployLock` 共用 `~/.escode/server/.deploy.lock`；修复 heartbeat 子 shell 继承 SSH channel fd 导致 release 等待超时的根因（heartbeat 重定向 `</dev/null >/dev/null 2>&1`）。
  - `SourceServerConnections` 显式连接在启动 server 前安装：发布 `needs-install`（含 `installPreview`）→ `bootstrapping` → `online`；`ConnectIntent::Recover` 不探测/不安装。失败发布 `failed` + `remote-preflight-failed` / `remote-download-failed` / `remote-upload-failed` / `remote-install-failed` / `remote-space-insufficient` / `remote-permission-denied`，不发布 `online`。
  - Main/TS：`rpcListen.remoteAssets`（releaseDir/cdnBaseUrls/appVersion）由 `resolveRustHostRemoteAssets` 生成并经 `RustHostManager` 懒取，Host 组合根构建 `RemoteExecutor` 与安装配置。远端 scope attachment 以「capability 签发成功（Host 建连/安装完成）且 WebSocket 打开」为就绪边界，端口交付后才开始 renderer attachment 握手计时；capability 失败作为显式 transport 失败上抛，不再泄漏成未决 attachment。
  - 验证：`cargo test -p escode-host-services` 627 passed；`cargo clippy -p escode-host-services -p escode-host-rpc -p escode-host-control --all-targets -- -D warnings`、`node scripts/check-escode-host-boundaries.mjs`、`pnpm check:escode-host`、`pnpm typecheck`、`pnpm lint`（210 基线下 0 error）、`pnpm architecture:check --changed`、`packages/desktop/test/rustHostRemoteScope.test.ts` + `rustHostBridge.test.ts`（22 passed）通过。
  - 真实 SSH（Docker fixture，Linux x64）：空容器 local-upload 首装 → 启动 server → hello/Initialize 成功、重复探测幂等（`source_server_ssh_install`，debug 构建 prepare ~67s / apply ~73s，gzip 为主要开销）；空容器 remote-download（本地仅 manifest + 本地 HTTP 提供 `mock-cdn/components`）7.5s 完成安装并幂等。Desktop formal E2E `desktop-e2e-20261030-215217-709`（127.0.0.1:22225 空目标）`1 passing (2m 41.1s)`：记录到 `needs-install` 与 `installPreview.sizeBytes > 0`/`version=3.15.2`，operation log 含 `installed source server 3.15.2 for linux-x64`，远端 `~/.escode/server/node` / `zcode-server.cjs` 存在，进程表无 `escode-host-remote-*`。该用例的 Mocha 上限在 `wdio.conf.ts` 单独放宽到 600s（默认 120s 会在 debug 首装完成前截断；用例内 `this.timeout` 无法覆盖 config）。
  - 已知缺口：UI 侧的安装确认（`source-server-install-required`）目前仍无 Host 生产者；显式连接按 §11.3 直接安装，确认弹窗留待后续 UX 切片接入（Node 路径同样未产出该错误码）。
- 已落地（2026-10-31，S4-d）：
  - 新增 `source_server_update` / `source_server_update_install` / `source_server_update_flow`：`updateSource` 严格解析 `profileId` / 可选 `force` / 可选 `expectedTarget`，按不可变执行身份（SSH host 小写 + 默认 22 + username + alias；WSL 必须含 distro/user；Docker container）校验；目标变化返回 `remote-target-changed`，目标只含身份字段、不带凭据。
  - 更新前按 Source 声明 workspace 调远端 `escode-task.listTaskList`（`kind:"active"`、`workspaceScopes`、`limit:null`）只保留 running 行；有任务且 `force:false` → `source-server-update-running-tasks`；**清单不可读同样按 guard 拒绝**（不把未知当成空），必须显式 `force:true`。远端从未安装 server 时无需临时连接。
  - 更新始终安装 manifest 全量组件（与首连只装缺失组件不同）：部署锁内先备份 `~/.escode/server` 到 `~/.escode/.update-backup-<uuid>`，写 glm `pendingRefreshAppVersion`，再安装；新连接 hello/Initialize/兼容验证成功后才 commit（丢弃备份、释放锁）。安装或握手失败恢复旧 release、释放锁并恢复旧连接；旧连接在换代前 dispose，scoped attachment 按 §10.14 收口。
  - 失败投影保留操作前状态（online 仍 online），只附加 `error` / `errorCode` / `retryable` / `runningTasks`，旧任务列表不残留；只有握手不兼容才进入 `needs-upgrade + source-server-upgrade-required`。
  - RPC：`updateSource` 返回管理快照并透传 `code` + `retryable`；`host-proto` 实现清单登记 `updateSource`；`session.rs` 的 passthrough 字段加入 `retryable`。
  - 验证：`cargo test -p escode-host-services` 643 lib passed（新增 9 个纯单测 + 7 个连接/flow 测试：running-task guard→force、target 变化不出 transport、清单不可用需 force、prepare 失败保持 online、启动失败回滚 release 并恢复旧连接、成功后刷新版本/online 且旧 client 关闭/新 client 不同、recovery intent 不安装）；`cargo test -p escode-host-rpc -p escode-host-proto` 通过；`cargo clippy -p escode-host-services -p escode-host-rpc -p escode-host-proto --all-targets -- -D warnings`、`node scripts/check-escode-host-boundaries.mjs`、`pnpm check:escode-host`（51 channels / 565 members）、`pnpm typecheck` 通过。
  - 真实 SSH formal E2E `desktop-e2e-20261030-224107-752`（127.0.0.1:22225 空目标，debug release）：首装后 `force:true` 更新到 3.15.2，`1 passing (5m 14.7s)`；断言出现 `updating` 状态、`currentVersion=3.15.2`、operation log `source server updated to 3.15.2`、远端 `.asset-components` 元数据无 `pendingRefreshAppVersion`、无 `.update-backup-*` / `.deploy.lock` 残留、进程表无 `escode-host-remote-*`。E2E 以轮询 Renderer 结果替代单次长 `executeAsync`，避免 WebDriver 30s HTTP 请求超时（仅放宽 browser script timeout 不能绕过该层）。
- 待落地：S3 剩余——Windows/macOS E2E；S4 剩余——S4-i Windows/macOS 桌面 E2E（S4-a..S4-h 已完成，交叉编译与平台审计见 §11.11），设计与切片见 §11；S5——2026-10-31 更新：S5-a/S5-b 已完成（直接首连与本地开关收窄），S5-c/S5-d 保留 Web `kind: "server"` 自连路径，待退役 `/api/connect-remote` 后再删除，设计与删除顺序见 §12。

### 10.10 连接管理器切片（S3-a：profile → 连接事实 + 状态机）

> 设计记录：2026-10-30。本节只交付「一条 Connection Profile 的建立、状态投影与收口」，不交付目录绑定、remote attach 与 continuous 投递。

所有权：

| 状态                                       | owner                     | 备注                                                                                                      |
| ------------------------------------------ | ------------------------- | --------------------------------------------------------------------------------------------------------- |
| profileId → 活动连接、per-profile 尝试代次 | `SourceServerConnections` | `host-services::source_server_connections`；Host 内单例                                                   |
| 连接状态快照、事件 generation              | `SourceServerRegistry`    | 连接管理器只调用 `set_connection_state`，不另存一份状态副本                                               |
| transport 启动命令                         | 注入的 resolver           | 生产 = `parse_target` + `build_transport_command`；测试注入 fixture 命令（同 `Instances::with_link_for`） |
| 远端目录 / release / 鉴权                  | Source Server             | 本切片不实现 deploy / preflight / 兼容清单（S4）                                                          |

状态映射（`sourceServerConnectionStateSchema`）：

```text
absent ──connectProfile──► bootstrapping ──hello + Initialize──► online
                               │                                   │
                               ├─ 启动/握手失败 ──► failed          └─ 链路 EOF/写失败 ──► offline
                               └─ user disconnect ──────────────────────────────► absent
```

- `bootstrapping` 在 transport 启动前写入；`online` 只在 `RemoteChannelClient::wait_initialized()` 完成后写入，并带 `currentVersion = hello.version`。
- 握手超时 → `failed` + `errorCode = remote-connection-timeout` + `retryable = true`；其余启动/握手/EOF 失败 → `failed` + `retryable = true`，无法归因的 `errorCode` 不编造。
- 已 `online` 的链路随后断开 → `offline`（保留可读历史），`retryable = true`；不自动重试，重试入口仍是 Renderer 手动入口或 S4 恢复策略。
- `disconnectSource` → 该 Source 的所有活动 profile 写 `absent`，并 await 子进程退出。
- 运行态状态覆盖 settings 派生的 S2 投影（`failed`/`absent`）；Host 重启后运行态为空，重新按 settings 投影（S4 再引入持久化恢复）。

时序与代次：

```text
connectProfile(profile) ─► slot.starting 单飞 ─► attempt += 1 ─► 旧连接 shutdown
          └─ 写 bootstrapping ─► resolve transport ─► connect() ─► wait_initialized
                 ├─ 失败：写 failed，返回错误
                 └─ 成功：写 online，登记关闭 watcher（携带 attempt）
watcher(attempt) ─► 链路关闭 ─► attempt 仍当前？ ─是─► 取走连接并 shutdown，写 offline
                                              └否─► 丢弃（过期结果不得覆盖新一代）
```

- 同一 profile 的并发 `connectProfile` 只启动一次，后到者等锁后复用已知活动连接。
- 关闭 watcher 与 `disconnectSource` 都以 attempt 判序；`disconnectSource` 先递增 attempt 再写 `absent`，因此收口过程不会把 `absent` 覆盖成 `offline`。
- 每次状态写入由 registry 递增 `generation` 并广播 `SourceServerEvent::State`，载荷严格等于 `sourceServerManagementStateDeltaSchema`。

RPC（`source-server-management`）：

- `connectProfile(profileId, {retainedOnly?})`：成功后返回 `getSnapshot()`；失败返回 RPC 错误（`Error`），同时已发布 `failed` 状态，调用方从快照读错误码。`retainedOnly` 只是恢复入口的过滤提示，当前无调用方，连接语义不变。
- `disconnectSource(sourceServerId)`：返回收口后的 `getSnapshot()`；未知 Source 不报错（无活动连接即幂等成功）。
- 未知 `profileId` → `InvalidRequest`，不创建空状态。

非目标与边界：

- 不把远端 Source Server 当成 NDJSON runtime 放进 §10.2 的 `Instances`：两者帧面不同，远端连接由本管理器持有。
- 不改变 Renderer 的 remote attach 选择；未被 Rust 接管的 workspace 仍由 Node Host 服务。
- 远端启动命令由调用方注入；默认值与 TS `buildRemoteServerCommand` 对齐留到 S4 部署切片。

### 10.11 远端 scope 转发（S3-b：Host 提供远端 workspace 服务面）

> 设计记录：2026-10-30。S3-a 只建立 profile 连接；本切片让 Rust Host 真正**服务**一个远端 workspace 的 RPC 面，仍不切换 Main 的 attachment 选择（Node Host 远程路径继续工作）。

目标形态：渲染器对远端 workspace 的服务面由 Rust Host 提供，Host 把该连接的每个 channel 调用转发到 Source Server 的 stdio RPC（§10.3 选型 B）。Node Host 远程路径删除时，渲染器不再需要 MessagePort→Node UtilityProcess。

**Scope 的所有权与授权**

- 一个 RPC WebSocket 连接绑定 0 或 1 个远端 scope：`{sourceServerId, profileId, workspaceIdentity}`。
- Scope 由 Main 在申请 capability 时声明：`POST /api/rpc-host-capability`（进程密钥头）body 可带 `{"scope": {...}}`。
- Host 先按 S3-a single-flight 建立/复用该 profile 的连接（`Initialize` 完成），成功后签发**绑定该 scope 与客户端句柄**的一次性 capability；签发失败（transport/握手/未知 profile）返回显式错误，不签发可回退的本地 capability。
- `/ws`、`/ws/host` 升级时消费 capability，得到连接级 `RemoteGrant { scope, client }`；未带 scope 的 capability 仍是本地 scope（现有行为不变）。

**Channel 集合（安全边界）**

```text
本地 scope 连接：共享 entries（全部本地域）+ 连接级单 channel（window-controller）
远端 scope 连接：Host 本地允许表 + 远端 Initialize 广播的 channel 转发入口
```

- 远端 scope **不得**出现本地文件/设置/终端/git/credential 等共享 channel：这些 channel 操作 Host 本机路径，暴露给远端 workspace 会把本机文件面泄漏给远端窗口。允许表由组合根显式给出（本切片只有 `window-controller`），未列入即不注册（`NotImplemented`）。
- 转发入口只在远端 `Initialize` 广播过该 channel 时创建：不猜远端能力；远端广播但不在 `host-proto::TS_SERVICE_CHANNELS` 里的 channel 跳过并记 warn。
- 成员名以 `on` + 大写开头的按事件转发（`listen`），其余按方法转发（`call`）；不在清单里的成员仍是显式缺口。

**转发语义**

| 项     | 行为                                                                                                                     |
| ------ | ------------------------------------------------------------------------------------------------------------------------ |
| 方法   | `RemoteChannelClient::call(channel, method, arg)`；返回值按 `WireValue`→`Wire` 转回                                      |
| 事件   | `listen(channel, event, arg)`；返回的流被丢弃（连接关闭/取消订阅）时 `dispose_listener`，不泄漏远端订阅                  |
| 事件序 | EventListen 必须在 handler 返回前写入远端出站队列（eager），保证先于紧随其后的 `call` 到达远端（见 2026-11-01 落地记录） |
| 错误   | `RemoteCallError{name,message,data}` → `RpcError{name: 远端 name, message, extra.code: data.code}`；不吞错、不重试       |
| 取消   | PromiseCancel 只中止本地等待任务；远端请求无取消帧时保持既有语义（与 TS `ChannelClient` 一致）                           |

**落地记录（2026-11-01：EventListen 必须 eager 入队）**

- 现象：macOS CI（`pnpm test:escode-host`，run `41277156616`）上 `v4_frames_match_the_node_remote_oracle`
  在订阅阶段 5s 无帧后超时。
- 根因：`remote_scope_listener::listener_stream` 用 `stream::unfold` 惰性创建，`client.listen(...)`
  只在流第一次被 poll 时才发出 EventListen；而读循环对每个帧都 `tokio::spawn` 处理，紧随其后的
  `subscribeConversationV4`（`call`）可以先被 poll 并写到远端。远端先收到 subscribe，立即投递的
  帧（fixture 用 `setImmediate`，真实 server 同语义）此时没有监听者，continuous 链路不重发 → 帧永久丢失。
- 修复：`RemoteChannelClient::listen_now` 在已 `Initialize` 时同步登记事件接收端并 `send_request`；
  `event_handler` 在 handler 调用栈内（`dispatch` 的到达序）就用它建立流。未完成握手时才退回惰性路径。
- 回归：`remote_scope_tests::event_listen_is_enqueued_before_a_following_call` 断言先构造事件流、
  再发 `call`，出站队列的第一帧必须是 EventListen。
- 顺序保证的边界：只保证「同一连接读循环里先到达的 listen 先入队」；`call` 的应答仍是异步的，
  Host 不缓存/重放远端帧。

**落地记录（2026-10-30）**

- 组合根装配：`domains::channels_with` 用 `Channels::with_remote_scope(remote_scope_ensurer(connections), REMOTE_SCOPE_HOST_CHANNELS).with_scoped_set(remote_scope_channel_set())`；`Channels::connect` 对远端 scope 先过滤共享 entries、再按工厂追加转发入口（`window-controller` 作为连接级 channel 仍在允许表内保留）。
- `capability::issue_for` 是唯一的签发入口；本地 scope 走 `Capabilities::issue()`，远端 scope 走 `issue_grant(CapabilityGrant{remote})`。
- 已知限制（S3-c 处理）：`RemoteGrant.client` 是签发时的句柄快照；链路断开后该连接的转发请求 fail-closed（`ConnectionClosed`），重连需要 Main/Renderer 重新申请 capability。转发面不缓存远端状态、不自动重试。

**不在本切片**

- Host 侧覆盖服务：`escode-task` 的 controller 路由、`media-preview` 远端代理、`file-watcher` 合流（S3-c）。
- Main 的 attachment 选择与 Renderer 的 scope 绑定（S3-c：Main 改向 Rust Host 申请远端 attachment）。
- 目录绑定写入、`desktop-continuous`/`web-remote-replayable` 投递差异（S3-c/S4）。

**验收**

1. 单测：方法转发帧、事件订阅与销毁、错误 code 透传、`Wire↔WireValue` 往返（含 100 KB 字节串）。
2. RPC 面：带 scope 的 capability 只能拿到允许表 + 远端 channel；对本地 `setting` channel 的请求回 `NotImplemented`；不带 scope 的连接行为不变。
3. 跨语言：真实 TS `ChannelServer` fixture 经 Host `serve()` + scoped capability 完成一次往返与一次事件。
4. 连接失败时 capability 签发失败（显式错误），不签发本地权限的 capability。

### 10.12 Main 申请远端 scope attachment（S3-c-1）

> 设计记录：2026-10-30。S3-b 让 Rust Host 能**服务**一个远端 scope，但没有调用方：Main 仍把远端
> workspace 的 `AttachServicePort` 发给 per-window Node Host。本切片让 Main 能按 Connection Profile
> 直接向 Rust Host 申请远端 attachment，并保留「同一 workspace 只有一条在线路径」的约束。

**入口与 owner**

- Renderer 现有的 `connectRemote(target, …)` 契约不变；`RemoteTarget` 的 ssh/wsl/docker 三种形态新增
  可选 `sourceServerId` / `profileId`：携带时表示「这条连接由 Connection Profile 描述」，其余字段只是
  展示与历史恢复用的脱敏快照（`buildRemoteTargetFromProfile` 负责填充）。
- 携带 `profileId` 且 Rust Host 可用 → Main 走 Rust 路径；S5-a 起未携带 `profileId` 的
  ssh/wsl/docker 也先由 Rust Host 匹配/创建 Profile 并回填 `sourceServerId`。仅 `kind: "server"` 的
  Web 服务端目标保留 Node/HTTP 路径。**兼容失败（profile 未知、transport/握手失败、
  Rust Host 不可用）显式报错，不回退 Node Host，也不改用 Node Host 服务同一 workspace。**
- 状态 owner 不变（§10.1）：连接事实、连接状态、实例仍归 Rust Host；Main 只持有
  `sessionId → {descriptor, renderer attachment}` 的路由表。

**Main → Rust Host 的两次调用**

```text
Renderer ──connectRemote(target{profileId})──► Main
Main ──POST /api/rpc-host-capability {scope}──► Rust Host   （Host 内部 connectProfile + Initialize）
     ◄── 200 {capability} | 4xx {error}                     失败：显式错误，不签发
Main ──WS /ws (web-remote-replayable) + capability──► Rust Host   （字节桥接到 MessagePort）
Main ──ScopedServicePort──► Renderer（协议不变，仍是 @escode/rpc）
```

- `ScopedServicePort` 元数据除 `attachmentId` / `sessionId` / `target` 外，必须携带 Main 的权威
  workspace scope：`sourceServerId?`、`workspacePath`、`workspaceIdentity`（与
  `scopedServicePortPayloadSchema` 一致）。Renderer 注册 session 时原样写入
  `sessionsById[sessionId]`；设备管理、项目打开和按 identity 查找 attachment 都只消费这些字段，
  不按 target 或 identity 字符串反推 Source 身份。首次 connect 与目录 bind 换代都必须携带。
- scope 在**首次连接**使用临时 identity `buildRemoteWorkspaceIdentity("/", target)`（与 Node Host 远
  程路径的 `workspacePath ?? "/"` 语义一致）；目录绑定后 Main 用新 identity 重新申请 capability 并
  换代 attachment——`RemoteGrant` 绑定签发时的句柄与 identity，不复用旧令牌。
- `issueRustHostCapability` 把 Host 的 `error` 文本透传给调用方；HTTP 4xx 不重试、不降级。
- 交付模式：桌面远端窗口（SSH/WSL/Docker/Connection Profile）与 Node 路径一致，使用
  `desktop-continuous`（`/ws/host`）；手机/Web 的共享 Host attachment 仍使用 replayable（`/ws`）。
  模式由调用方显式声明，不能按进程或 transport 猜测（S3-c-3b，§10.15）。

**route 归属**

`RemoteAttachmentRoute` 增加 `owner: "node-host" | "rust-host"`：

| 行为          | node-host（现状）                           | rust-host（本切片）                                                    |
| ------------- | ------------------------------------------- | ---------------------------------------------------------------------- |
| 建连          | `ConnectRemoteWorkspace` → Host `Connected` | Main 申请 scoped capability（Host 内建连）                             |
| attach        | `AttachServicePort` → Node Host             | `attachRemoteScopePort(scope, "desktop-continuous")` → `/ws/host`      |
| bind 目录     | `BindRemoteWorkspaceContext` → Node Host    | 更新 descriptor + 用新 identity 换代 attachment                        |
| provider 准备 | Node Host `ProviderProvisioningExecute`     | S3 跳过；S4 起由 Rust Host 下发本地配置到远端（§11.7，修正此处旧假设） |
| 连接关闭      | Node Host `RemoteSessionClosed`             | Bridge socket 关闭 → route `closed`（Host 重连后重新申请）             |
| 释放 session  | `DisposeRemoteWorkspaceSession`             | 只释放 attachment（实例按 §10.6 跨窗口共享，不随窗口关闭停止）         |
| 窗口关闭      | 同上 + 取消 pending                         | 只释放该窗口的 attachment 与 pending                                   |

**已知限制（S3-c-2/S3-c-3 处理）**

- 远端 scope 的 channel 集合目前只有 `window-controller` + 远端转发（§10.11）；`media-preview` 远端
  代理、`file-watcher` 合流、`escode-task` controller 路由仍需后续切片，否则远端窗口的部分本地服务
  会拿到显式 `NotImplemented`。
- 目录绑定依赖 `source-server-workspace.resolveWorkspace`（Rust Host 仍 `NotImplemented`），本切片只
  保证「Renderer 提供目录 → 重新申请 scope」这条链路。
- 断线重连的**自动恢复策略**（Host 启动按 `lastOnlineAt` 连接、自动重试预算）属于 S4；S3 已保证 Renderer 手动重连（含 App 重启后的断连 tab）保留 `profileId`/`sourceServerId` 并重新向 Rust Host 申请 capability，不再回退 per-window Node Host。
- 本切片交付 Main→Rust Host 的 attachment 路由与单元级桥接；三平台 E2E 与断线恢复仍由后续切片补齐。

**验收**

1. `issueRustHostCapability` 携带 scope 时 POST body 为 `{"scope":{sourceServerId,profileId,workspaceIdentity}}`；
   Host 4xx 时抛出 Host 的 `error` 文本（不回退、不重试）。
2. `attachRemoteScopePort` 在假 Host 上完成：capability → RPC WebSocket（桌面 `/ws/host`）→
   MessagePort 收发的双向帧桥接，并在 `dispose()` 后关闭 socket 与端口。
3. 远端 route 的 `connect → attach → bind(换代) → dispose` 不触碰 `windowHostProcessMap`（Node Host 进程零调用）。
4. 失败路径：scoped capability 失败时 `connectRemote` 抛错，且不创建 route、不调用 Node Host。

### 10.13 远端 window-controller（S3-c-2a）

> 设计记录：2026-10-31。S3-c-1 的远端 scope 只注册 Host 本地 `window-controller`；该控制器按 tasks-index
> 读本机数据库，远端 identity 的 scope 不贡献行，导致远端窗口的全局任务列表为空、`mutateTask` 报
> “没有与任务地址匹配的 source”。本切片让远端 scope 使用**连接级**控制器，任务事实仍来自 Source Server。

**所有权与数据流**

- 任务事实 owner 不变：`escode-task`（Source Server）是列表、membership、未读与归档事实的唯一 owner；
  Host 只持有每条远端连接的投影（rows/topic 水位/订阅表），不缓存第二份可写状态。
- 远端 scope 不再复用 Host 级本地 controller：`remote_scope_channel_set` 为每条远端连接构造
  `RemoteController`（`Projection` + 连接订阅表）；同名 channel 以连接级实现优先。
- 本地 workspace 行为不变：共享 `window-controller` 仍由 Host 级 controller 服务。

**读取与事件**

| 项                    | 远端行为                                                                                                                                          |
| --------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| 刷新                  | 调远端 `escode-task.listTasks` / `listPinnedTasks` / `listArchivedTasks`（带 `workspacePath`/`workspaceIdentity`），membership 合并口径与本地一致 |
| 变更信号              | 订阅远端 `escode-task.onDynamicWorkspaceEvent({workspacePath, workspaceIdentity})`，合并脏标记后续跑一轮刷新                                      |
| `listTaskList` 搜索   | 调远端 `escode-task.listTaskList`（scope 收敛到本连接），结果与投影行叠加；失败只记录并返回空，不猜数据                                           |
| `listTaskList` 非搜索 | 只读本连接投影 rows（membership 过滤 + 排序分页），不读本机 DB                                                                                    |
| 写操作                | `mutateTask` / `deleteArchivedTask(s)` 转调远端 `escode-task` 对应写面，成功后强制刷新投影并返回投影 meta；`open`/`resume` 只校验不写             |
| 地址                  | 行地址带 `sourceServerId` + stable `workspaceIdentity`；不使用 `remoteSessionId`（Rust Host 无会话注册表）                                        |
| 失败                  | 远端调用失败向调用方显式返回；刷新失败保留上一份可信投影并记录 warn，不回落本机 tasks-index                                                       |

**作用域隔离**

- 每条连接只服务 capability 绑定的 `workspaceIdentity`；查询其它 scope 不贡献行，写入其它 scope 返回
  “没有与任务地址匹配的 source”。同一远端 workspace 的多窗口各自持有订阅表，共享同一 Source Server 事实。
- 远端 identity 的路径从 stable identity 解析（`remote:server:<sourceServerId>:<posixPath>`）；解析失败时
  不猜测路径，直接按 identity 原文拒绝写入。

**已知限制（后续切片）**

- `media-preview` 在远端 scope 仍转发到 Source Server 并走 inline（≤ 8 MiB）；桌面 attachment 改为
  `desktop-continuous` 后也不需要 Host loopback Range 代理（文件事实在远端，见 §10.9 S3-c-2b 结论）。
- `file-watcher` 已由远端 channel 转发；Host 侧与本地 watcher 的合流仍待本地 overlay 需求明确后处理。
- `source-server-workspace.resolveWorkspace` 的 canonical alias 属于 Source Server Core 权威（R3 架构），
  Host 侧只保留 registry 目录事件；接入远端 Core 时再补。

**验收**

1. 跨语言 fixture：远端 scope 连接上的 `window-controller.listTaskList` 返回 fixture 任务，行地址包含
   `sourceServerId` 与 stable identity。
2. `mutateTask`（pin/archive/unread）转调远端 `escode-task` 写面，刷新后的投影 membership 与远端一致。
3. 远端 scope 只服务绑定的 workspace；本地 scope 的 controller 行/帧不变。
4. `subscribeControllerV4` 在远端刷新后投递含任务行的快照；事件触发增量帧。
5. 远端缺少 `escode-task` channel 时返回显式错误，不回落本机 DB、不返回伪造空列表。

### 10.14 远端链路断开传导（S3-c-3a）

> 设计记录：2026-10-31。S3-a 在远端链路结束时只把 registry 写 `offline`；绑定该连接的 RPC WebSocket
> （`/ws` 或 `/ws/host`）仍保持打开，Main 的桥也只记录 socket 关闭。结果是 Renderer tab 继续显示“已连接”，后续调用以
> `ConnectionClosed` 失败，既不进入断连态，也没有手动重连入口。本切片只补“断线事实如何到达窗口”，
> 不做自动重试。

**所有权与事件顺序**

- 链路事实 owner 仍是 Rust Host 连接管理器（§10.10）：`RemoteChannelClient::dispose()`（reader/writer EOF、
  解码失败、显式 `shutdown`）就是该连接所有 scope 的终态信号。
- Rust Host 在 scope RPC 连接上等待该信号；信号到达即结束连接（关闭 WebSocket），不伪造业务错误帧。
  本地 scope 连接没有远端 grant，不参与该等待。
- Main 把 attachment 桥的**非主动**关闭映射为 route `closed`，并向该窗口发送既有
  `RemoteSessionClosed`；主动 `dispose()` / bind 换代 / renderer 取消不发送事件。
- Renderer 沿用现有 `onRemoteSessionClosed`：清 `remoteSessionId`、标 `disconnected`，重连仍走既有
  `connectRemote`（同一 workspace identity 重新申请 scope）。不新增协议字段，不改 V4 帧。

```text
Source Server 链路 EOF
  → host-services: RemoteChannelClient.dispose() + registry = offline
  → host-rpc: grant 绑定的 /ws 看到 wait_closed，结束连接
  → Main: bridge 非主动 close → route.attachmentState = closed
  → Renderer: RemoteSessionClosed { reasonCode: "source-server-offline", retryable: true }
  → 用户/自动恢复入口 connectRemote(同 target) → 新 capability + 新 /ws
```

**边界**

- Host 退出（`RustHostExit`）走 `onLost`，远端链路断开走 `onClosed`；两者分开通知，避免进程退出时
  socket 先关闭把原因误判成 Source 断链。Host 退出与链路断开都只收口一次 route。
- 不自动重连、不保留跨断线的旧 scope：重连必须重新签发 capability（§10.11 的 grant 是句柄快照）。
- 断线后不删除 workspace 历史，也不把远端仍可能运行的任务写成本地 failed。

**验收**

1. Rust：`RemoteChannelClient` 被 dispose 后，绑定 grant 的 RPC WebSocket 在超时内关闭；本地 scope 连接不受影响。
2. Main：桥收到非主动 socket 关闭时 `onLost({reason:"socket-closed"})` 只触发一次；调用 `dispose()` 不触发。
3. Main：rust route 收到该事件后标 `closed` 并发送 `RemoteSessionClosed`（`source-server-offline`），
   Node Host 进程零调用。
4. 通知后同一 target 可重新 `createRemoteWorkspaceSession`；旧 route/attachment 已释放。
5. Host 退出路径复用同一 route 收口，不重复发送两次事件；`onLost` 仍以 `host-exited` 通知消费方。

### 10.15 投递模式边界（S3-c-3b）

> 设计记录：2026-10-31。S3-c-1 把 Rust 远端 attachment 写成 `web-remote-replayable`，沿用了
> A9 时期「Rust Host 只服务手机共享 Host」的假设。S3-c-1 之后同一路径也服务桌面远端窗口，
> 如果继续用 replayable，会与 R3 架构和 Node 远端路径的桌面 `continuous` 语义分叉。本切片把
> 模式交还给调用方，并用跨语言测试固定「Host 只转发、不改写 Source Server 投递事实」。

**产品边界**

| attachment                                 | clientMode              | 路径       | 说明                                    |
| ------------------------------------------ | ----------------------- | ---------- | --------------------------------------- |
| 本地桌面窗口                               | `desktop-continuous`    | `/ws/host` | 现有行为不变                            |
| SSH/WSL/Docker/Connection Profile 桌面窗口 | `desktop-continuous`    | `/ws/host` | 与 Node 远端路径一致                    |
| 手机 `/remote` 共享 Host attachment        | `web-remote-replayable` | `/ws`      | 现有行为不变                            |
| Web remote / relay WebSocket               | `web-remote-replayable` | `/ws`      | 由 Source Server 直接服务，不经过本路径 |

**数据流**

- 桌面远端窗口的本地 RPC attachment 使用 `/ws/host`；Main 显式传入 `desktop-continuous`，
  `attachRemoteScopePort` 不再提供隐式默认值。
- `escode-agent`、`escode-task`、`escode-session` 仍在远端 scope 内全转发到 Source Server
  （R3 架构 §「服务集合落点」）；Rust Host 不本地构造第二份 V4 投影，也不把 replayable 的
  snapshot/gap 逻辑拼进 continuous 链路。
- Source Server 侧的 stdio 连接继续是 `desktop-continuous`；因此 `hello.clientMode`、
  `deliveryProfile`、`inputOrigin`、`ttft` 等字段由 Source Server 按自身连接语义产出，Rust Host
  只做 Wire 帧透传。
- 断线时仍是 §10.14 的收口路径：链路 EOF → `/ws/host` 关闭 → `RemoteSessionClosed`；
  重连重新签发 capability，不跨断线复用旧 grant。

**验收**

1. Main 组合根对桌面远端窗口调用 `attachRemoteScopePort(..., "desktop-continuous")`，桥接到
   `/ws/host`；手机/Web 共享 Host attachment 的 replayable 断言保持。
2. 跨语言 fixture 注册 `escode-agent` 后，经 `/ws/host` 的 scoped capability 调用
   `helloConversationV4`，返回的 `clientMode`/`deliveryProfile` 与 fixture 的 continuous 配置一致；
   订阅帧与命令参数不被 Rust Host 改写。
3. 同一 fixture 以 replayable 配置运行时，经 `/ws` 返回 `web-remote-replayable`/`replayable`，
   证明 Host 没有把某个模式写死。
4. `window-controller` 与 `escode-task` 的远端读写、事件在两种模式下行为不变。

### 10.16 Node/Rust 远端 V4 帧差分（S3-c-3d）

> 设计记录：2026-10-31。§10.15 证明了 Rust Host 不按路径写死投递模式，但没有回答「同一 Source
> Server 事实经 Node Host 与 Rust Host 后，客户端看到的 V4 帧是否一致」。本切片用同一份 TS fixture
> 分别驱动两条远端链路，把客户端可见的 hello、订阅 ack、事件帧和命令 ack 规范化后逐字段对拍。

**链路定义**

| 路径 | 链路                                                                                                                                              |
| ---- | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| Node | fixture(stdio) ← `ChannelClient`（Node 远端连接）← `ChannelServer` 转发（等价 per-window Node Host attachment）← `ChannelClient`（renderer 视角） |
| Rust | fixture(stdio) ← `SourceServerConnections` ← scoped capability `/ws/host` ← V4 调用序列                                                           |

Node oracle 与 Rust 测试执行完全相同的 V4 场景：

1. `helloConversationV4`；
2. `onDynamicConversationFrame` 监听 + `subscribeConversationV4`；
3. `sendConversationCommandV4`（`sendText`，带 `sessionId`）。

**规范化**

- 只比较语义字段：`hello`、订阅 ack、按 `kind` 排序后的事件帧、命令 ack。
- 忽略 Source Server 的非确定字段：`hello.serverTime`、`hello.connectionId`。
- 帧保留 `topic` / `subscriptionId`：两者是 Source Server 对同一 workspace/session 的权威输出，
  Host 改写会改变客户端订阅归属。
- 顺序差异只来自 fixture 的 `setImmediate` 投递，不视为语义差异，因此帧按 `kind` 稳定排序。

**边界**

- fixture 只固定「单 session 最小直通路径」，不宣称替代真实 Source Server 的完整 V4 差分。
- 差分失败时测试直接失败并输出两侧 JSON；不允许把 Rust 路径的差异归因成 fixture 噪声后跳过。
- 该测试覆盖桌面 remote 的 `desktop-continuous`（`/ws/host`）与手机/Web 共享 Host 的
  `web-remote-replayable`（`/ws`）两种 fixture 配置，确认 Host 没有按路径改写投递事实。

**验收**

1. 同一 fixture 下，Node oracle 与 Rust `/ws/host` 的规范化 JSON 完全一致（hello、ack、snapshot/deltas
   帧、命令 ack）。
2. replayable 配置下，Node oracle 与 Rust `/ws` 的规范化 JSON 完全一致。
3. 帧中的 `inputOrigin`、`clientId`、`commandId`、`text` 未被 Host 改写。

**复跑记录（2026-10-31，S4-i 审计）**

- `cargo test -p escode-host --test source_server_remote_scope_ts` 初次复跑失败：S4-f 把
  `source-server-workspace` 纳入允许表后，`scoped_capability_round_trips_and_forwards_events_to_the_ts_server`
  的期望集合仍是旧的三项。根因是 S4-f 更新了转发实现与 fixture，漏改这条跨语言断言；已补上该
  channel（无实现改动）。
- 修复后 5 例全过，其中 `v4_frames_match_the_node_remote_oracle` 在 `desktop-continuous` 与
  `web-remote-replayable` 两种配置下继续逐字段一致；随后 `cargo test --workspace`
  （`apps/escode-host`）全绿。该结果作为「Node/Rust 远端同 fixture V4 差分」的当前证据，也是 S5
  删除前置条件之一。

## 11. S4 设计：部署、兼容、更新、恢复、迁移与 Provider 准备

> 设计记录：2026-10-31。S3 已让 Rust Host 成为「已部署 Source Server」的连接、实例与远端 scope
> owner；S4 补齐从零安装、版本兼容、显式更新、重启自动恢复、工作区迁移与 Provider 下发，使远端不再
> 依赖 per-window Node Host 的任何部署/恢复逻辑。Node Host 远程路径仍保留到 S5 删除。

### 11.1 所有权与状态扩展

| 状态                                                                   | S4 owner        | 备注                               |
| ---------------------------------------------------------------------- | --------------- | ---------------------------------- |
| release 解析、部署决策、preflight、安装/更新操作、兼容判定、恢复预算   | Rust Host       | Host 是连接与生命周期唯一 owner    |
| 远端目录、release 产物、安装落盘、运行中任务、迁移执行、Provider Store | Source Server   | Host 不复制远端业务事实            |
| 更新/重连用户动作、窗口 attachment                                     | Main / Renderer | Main 只转发触发，不持有远端状态    |
| Personal Provider Config、账号窄设置、allowlist OAuth 凭据             | 本地 Rust Host  | 唯一编辑权威；远端只保存可丢弃镜像 |

S3 的 `bootstrapping → online` 之外新增：

```text
absent ──connect / auto-connect──► bootstrapping
                                      ├─ release 缺失或身份不匹配 ──► needs-install
                                      ├─ 兼容不匹配 ─────────────────► needs-upgrade
                                      ├─ 更新执行中 ─────────────────► updating
                                      ├─ 静默恢复中 ─────────────────► retrying
                                      ├─ 指纹变化待确认 ─────────────► identity-change-pending
                                      └─ 显式失败 ───────────────────► failed
online ──链路断开──► retrying（2s → 4s → 8s，×3）──► offline
```

`ConnectionStateSnapshot` 补齐 `sourceServerConnectionStateSnapshotSchema` 的其余字段：
`updateAvailability`、`availableVersion`、`minimumVersion`、`upgradeTarget`、`installPreview`、
`runningTasks`、`pendingSourceServerId`、`recoveryAttempt`、`recoveryLimit`、`recoveryBudgetExhausted`。
字段只在事实存在时输出，缺失即省略，禁止写 `null`（§9.2）。

### 11.2 部署单元与来源

**决策：复用现有 component manifest，不引入第二套 release 格式。**

- 安装单元 = `packages/desktop/mock-cdn/releases/<appVersion>/manifest-<platformArch>.json`
  （`schemaVersion` / `appVersion` / `platformArch` / `components[]`）。
- 每个 component 已带 `id`、`version`、`sha256`、`artifactPath`
  （`components/<platform>/<id>/<version>.tar.gz`）与 `mount`；当前为 `server-bundle`、`node-runtime`、
  `node-pty`、`glm`、`bfs`、`ripgrep`、`ugrep`。
- 远端布局保持 `~/.escode/server`：`node/`、`server/`（`zcode-server.cjs`）、`glm/`、`node-pty/`、
  `tools/`；组件身份标记沿用 `~/.escode/server/.asset-components/<id>.json`
  （`id` / `version` / `sha256` / `platformArch`）。
- 不采用 `packages/escode-server-cli` 的 `escode-server-<target>.tar.gz` + daemon 模型作为本轮部署单元：
  远端当前仍由 `desktop-attached` stdio server 提供服务，切换 daemon 会同时改变传输、重连与生命周期
  语义，超出「删除 Node Host 远程路径」的目标。后续若迁移到 R2/R3 daemon，
  `sourceServerReleaseDescriptorSchema` 作为独立工作包接入，不与本轮混用。

来源与策略：

| 运行形态             | release 来源                                   | 更新判定                                                     |
| -------------------- | ---------------------------------------------- | ------------------------------------------------------------ |
| 打包 Desktop         | 应用内置 release 目录 / CDN                    | `release-versioned`：按 manifest 版本 + component `sha256`   |
| 未打包（开发 / E2E） | 本轮本地 staging 的 release 目录，禁止回退 CDN | `development-content-addressed`：版本相同但 sha 不同也需更新 |

`assetInstallMode` 只决定产物在哪一侧获取：

- `local-download-upload`：Host 读本地 release 产物（或先下载到本地缓存），逐 component 经部署 transport
  stdin 上传。
- `remote-download`：远端按 `artifactPath`/URL 用 `curl`/`wget` 下载，本地与远端都校验同一 `sha256`。
- 两条路径在「远端临时目录 → 校验 → 解压到 mount → 原子替换」汇合，不允许第二套安装实现。

### 11.3 首次安装（needs-install）

```text
connect(profile)
  ├─ detect remote platform / arch
  ├─ resolve manifest（本地 release 或 CDN；开发态只读本地）
  ├─ preflight（remote-download）：curl|wget、tar、sha256sum|shasum|openssl
  │    └─ 缺失 → failed + remote-preflight-failed，提示切换 local-download-upload
  ├─ 采集 install preview（version / installPath / sizeBytes）
  ├─ acquire remote deploy lock（~/.escode/server/.deploy-lock，stale 600s）
  ├─ install components（staging → verify sha → move）
  ├─ write component meta
  └─ 启动 server → hello → Initialize → online
```

- 首次连接只在用户显式连接（`connectProfile` 或无 profile 的直接连接）时安装；自动恢复路径**不安装新版本**，
  缺失 release 时停在 `needs-install` 等用户动作。
- 安装失败保留旧 release（或空）并返回显式错误码：`remote-preflight-failed` / `remote-download-failed` /
  `remote-upload-failed` / `remote-install-failed` / `remote-space-insufficient` /
  `remote-permission-denied`。
- 部署锁语义与 Node `remoteDeployLock` 对齐（owner token + heartbeat + stale claim）；Rust Host 是唯一写入方，
  但必须与旧 Node 部署共用同一锁目录，保证混跑期间互斥。
- 部署锁 release 收尾（2026-11-01）：heartbeat 子 shell 的 SIGTERM 可能落在 dash fork 启动窗口内被丢弃
  （`kill` 返回 0，子 shell 却仍存活并持续刷新 owner 文件），旧脚本在 `kill` 后无界 `wait`，于是 release
  一直卡到超时。`74167f06` / `e11c680f` 的 `rust-runtime-ci` 失败即此根因，与并发负载无关：本地
  「读完 acquired 立即 release」48 并发 800 次挂起 16 次，同一脚本延迟 200ms 再 release 则 0 次。
- 修复为只 `kill` 不 `wait`：释放语义只取决于锁目录（owner 校验后 `rm -rf`），heartbeat 的 fd 已全部指向
  `/dev/null`，即使多活一个心跳周期也不占用 SSH 通道；锁目录删除后它下一轮 `cat owner` 失败即自行退出。
  回归用例 `immediate_release_after_acquire_does_not_wait_on_the_heartbeat`（300 轮紧邻 acquire→release，
  2s 预算；旧实现在本机每轮约 2% 命中）。
- 单测 `source_server_deploy_lock_tests` 的 release 预算仍取 30s，覆盖 CI 在 764 个并发用例里 spawn `sh`
  的慢启动；产品默认 `DEFAULT_RELEASE_TIMEOUT` 仍为 5s，超时只终止本机 holder、不删远端锁。
- 安装期间状态为 `bootstrapping` 并写 operation log；不发布 `online`，不签发远端 capability。

### 11.4 兼容清单与显式更新

- 远端 `escode-hello` 增加可选 `compatibility: {schemaVersion: 1, productVersion, minCompatibleVersion}`
  （`compatibilityManifestSchema.strip()`）。旧 server 不返回时视为「兼容未知」：允许连接但不推断兼容，
  Rust Host 不因缺失字段拒绝旧 server。
- 本地使用 `ESCODE_COMPATIBILITY_MANIFEST` 与 `evaluateSourceServerCompatibility` 的等价实现，映射：
  - 兼容且版本相同 → `updateAvailability: none`
  - 兼容但版本不同 → `updateAvailability: available`，连接继续
  - 不兼容且远端较旧 → `needs-upgrade` + `source-server-upgrade-required` + `upgradeTarget: server`
  - 不兼容且远端较新 → `failed` + `upgradeTarget: controller`（升级 Desktop，不自动降级）
- 连接流程**不得**直接执行安装/更新。更新只由显式 `updateSource(profileId, {force?, expectedTarget?})` 触发：

```text
updateSource
  ├─ 校验 expectedTarget（profile 未变才继续；否则 identity-change-pending）
  ├─ 读 running tasks（远端 escode-task.listTaskList，按 declaredWorkspaces 收敛）
  │    ├─ 有运行任务且 !force → source-server-update-running-tasks
  │    └─ 清单不可用 → 明确「不可用」，不推测为空
  ├─ updating + operation log（phase / progress）
  ├─ 安装新 release（同 §11.3；先写 pending，再原子切换）
  ├─ 停止旧连接 → 启动新 server → hello / Initialize
  │    ├─ 成功：online、updateAvailability=none、刷新 version / lastOnlineAt
  │    └─ 失败：恢复旧 release 与旧连接，原 online 事实不变，只附加更新错误
  └─ 取消（断开 / 删除 / 窗口释放）只终止本机操作，不承诺回滚远端已应用的 release
```

- `force:false` 先行；`force:true` 只在用户明确确认后调用。运行任务清单不可读时不得把「未知」当成「没有任务」。
- 更新失败不得把可用的在线 Source 改判为 `needs-upgrade`；只有握手兼容失败或开发态内容寻址不一致才进入
  阻塞状态。
- 更新成功后必须换代 `remoteSessionId` 与 attachment（复用 §10.14 收口路径），旧句柄全部释放。

### 11.5 重启恢复与自动重连

- 握手成功写 Source 级 `lastOnlineAt`；离开 online 时用 Host 时钟经 settings 串行写队列落盘。迟到写入不得
  覆盖更晚的握手，也不得重建已删除的 Source；离线重试不推进该时间。
- Host 启动后按 settings 恢复绑定，并对「近 7 天有 `lastOnlineAt` 的 Source + 已打开的断连 tab」发起自动连接：
  - ≤3 并发、每 Source 只试一次、单次 10s、静默（不弹凭据、不装新版本、不做兼容升级）。
  - 无可用凭据 / `needs-install` / `needs-upgrade` → 停在对应状态等待用户动作。
  - 实现边界：Host 只枚举近 7 天有 `lastOnlineAt` 的 Source（拒绝对同一个 flight 再开 Renderer 入口）；
    「已打开的断连 tab」由 Renderer 从 settings/tab store 恢复后，消费 Host `online` 事实换发
    `remoteSessionId`；Host 不持有窗口 tab 状态，超出 7 天窗口的 Source 仍保留手动重连入口。
- online 后链路断开：`retrying` 2s → 4s → 8s，最多 3 次；预算耗尽 → `offline` +
  `remote-recovery-budget-exhausted` + `recoveryBudgetExhausted: true`。离线后不自动探测安装/升级。
- 手动重连不伪造自动重试计数；自动恢复每次尝试都经 registry 发布，Renderer 显示
  `recoveryAttempt/recoveryLimit`。
- 重启后实例表不持久化进程句柄；懒启动新代次，旧代次只作记账与日志。

### 11.6 工作区迁移

- `source-server-workspace` 的 owner 是远端 Source Server：`resolveWorkspace`、`listMigrationReports`、
  `getFailedPathCount`、`getAbandonedPathCount`、`abandonFailedWorkspace`、`retryWorkspaceMigration`
  由远端执行。
- Rust Host 侧：
  - 提供 `source-server-workspace` 域，把调用转发到当前 scope / 默认 profile 的远端客户端；
  - 把 `failedPathCount` / `abandonedPathCount` / `migrationCounts` 投影进 `getSnapshot`；
  - `migrationCounts` 按 Source 标注 `available` / `requires-connection` / `loading` / `unavailable`，
    不把「未连接」当成 0 失败。
- 远端未实现或旧版本不支持时返回 `unavailable`，UI 只读显示「清单不可用」，不推测、不吞错。
- 迁移冲突沿用 `sourceServerMigrationConflictSchema`（`task-identity` / `config-file`）；部分失败按
  `failedPathCount` 展示，`abandoned-diverged` 保留 `abandonedTrigger`。
- 迁移是显式入口（`getMigrationReport` / `exportMigrationReport` 在 Host 侧聚合与导出），不在连接流程内
  自动改写远端数据。

#### S4-f 落地设计（2026-10-31）

- 投影 owner：迁移统计不是持久化事实，运行态只存在于 Host 管理面的迁移统计 owner（随
  `SourceServerConnections` 生命周期）；`getSnapshot` 由它读取 registry 目录后合并统计，不写入 settings。
- 物理连接 token：`(profileId, attempt)`。每次 connect / disconnect / 更新换代都会推进 attempt；
  统计结果只在 token 与当前首选在线 profile 一致、Source 仍存在且 owner 未释放时发布。
- 读取：每个 Source 至多一组 `getFailedPathCount` / `getAbandonedPathCount` 在途；两项都成功才算
  `available`，任一项失败或远端 channel 未广播 → `unavailable`。没有在线连接 → `requires-connection`；
  首次读取或 token 变化 → `loading` 并异步补全。顶层总数只累加 `available` 项。
- 刷新与失效：`getSnapshot({ refreshMigrationCounts: true })` 使已有投影失效（在途只标记一次
  `refreshRequested`，完成后重读一次）；默认 `false` 只读投影，首次读取仍会发起一轮异步读取。
  连接状态变化、Source 移除、迁移重试/放弃都会失效；统计完成或显式失效通过既有目录事件提升
  registry generation，不新增轮询。
- 迟到结果：读取完成时若 token 已变、Source 已移除、entry 已被替换或 owner 已 dispose，直接丢弃，
  不发布 generation。
- `source-server-workspace` 暴露：远端 scope 连接继续由既有 scoped 转发把远端广播的该 channel 原样
  交给 Renderer；本地连接没有目标 Source，禁止猜测默认设备，调用回显式缺口语义。管理面按
  `sourceServerId` 选择首选在线 profile 调远端，未连接返回 “Source Server must be connected”。
- 管理面新增：`getMigrationReport` / `exportMigrationReport`（`listMigrationReports` 的 Host 聚合与
  缩进 JSON 导出）、`abandonWorkspaceMigration` / `retryWorkspaceMigration`（转调远端并在 finally
  语义下失效统计、发布目录事件）。远端返回的报告原样透传，Host 不改写 `abandonedTrigger` / 冲突项。

S4-f 验收（契约 + 快照测试）：

1. 离线 Source 的 `migrationCounts` 为 `requires-connection`，顶层总数为 0 但不推断远端为 0。
2. 在线且远端延迟返回：首次快照为 `loading`；完成后目录事件 generation 递增，快照为 `available`。
3. 任一计数 RPC 失败或 channel 缺失 → `unavailable`（不是 0）。
4. 断线/换代后的迟到结果不发布；重连产生新 token 后重新读取。
5. `refreshMigrationCounts: true` 触发重读；默认 false 只读投影；未知字段/非布尔值显式拒绝。
6. 重试/放弃后统计失效并发布目录事件；导出报告为缩进 JSON + 结尾换行。

### 11.7 Provider 准备（修正 §10.12）

§10.12 曾把 Rust 路径的 provider 准备写成「跳过，远端由 Source Server 负责」。这是错误假设：远端
Source Server 只持有执行镜像，配置来源仍是本地 Environment；不 provisioning 时远端无法使用本地选中的
Personal Provider 与 OAuth 账号。

```text
Rust Host（本地）
  ├─ 读 Personal Provider Config（provider/files.rs + provider/personal.rs，revision 与文件同源）
  ├─ 读账号窄设置（providerFamilyDomain / providerFamilyConnectionSelections）
  ├─ 读 allowlist OAuth 凭据（credential::CredentialService）
  ├─ 组 ProviderProvisioningEnvelope(schemaVersion=2, syncId, ...)
  └─ 调远端 channel "provider-provisioning-target".apply(envelope)
        ├─ applied / already-applied → 允许发布 online
        └─ failed / rollback_failed / unsupported → 首连失败（正式连接失败通道）
```

- Envelope 只在 Host 内存中存在一次；不写日志、不进 Renderer、不持久化到 Main。
- 触发：环境首次 online（首连屏障）、Personal Config 保存、默认模型变化、账号窄设置变化、allowlist
  凭据变化。Host 是执行方；Main 只保留 environment lane / generation。
- 已 online 后的同步失败不拆连接，只记录不含 Secret 的结构化 warning；首连失败必须显式失败，不能发布
  ready。
- 远端 target 未注册（旧版本）→ `unsupported`，按兼容门先升级再重试，不跳过屏障。
- Rust 侧读取必须复用与 TS 相同的 allowlist 与 v2 校验；allowlist 缺失采用 replace 语义，非 allowlist
  凭据不参与同步。

S4-g 落地设计（2026-10-31）：

- Owner：`SourceServerProviderProvisioning` 只活在 Host 进程内（`ConnectionsOptions` 持有），不注册为
  RPC channel、不进 `ServiceCollection`、不经 Renderer 或 Main 转发。它只持有 source 读取器与 per-profile
  单飞 lane；活动连接仍由 `SourceServerConnections` 唯一拥有。
- 读取与 Envelope：Personal Config 使用 `ProviderService` 同一份加载快照校验 revision；文件缺失只允许空
  Personal（与 TS `readProvisionablePersonalConfig` 相同）。账号窄设置只取
  `providerFamilyDomain` / `providerFamilyConnectionSelections`。凭据只读取 8 个 allowlist key，逐条解密，
  空值跳过，非 allowlist key（含坏值）不参与也不阻断。信封固定 `schemaVersion: 2`，每次同步生成新
  `syncId`，只在 Host 内存存在一次。
- 首连屏障：`connect_with` 在 Initialize 成功后、发布 `online` 前调用远端
  `provider-provisioning-target.apply`。仅「显式连接且该 Source 从未记录 `lastOnlineAt`」是屏障；
  `applied` / `already-applied` 才继续 store + online。`failed` / `rollback_failed` / `unsupported`、
  读取失败或 channel 缺失均发布正式 `failed`（带固定 errorCode），关闭连接且不发布 online。
  `Recover`、重连与曾在线过的显式连接仍执行一次整份同步，但失败只记结构化 warning，不拆连接。
  - 2026-10-31 修复：握手记录曾把 `lastOnlineAt` 写成握手时间（早于屏障判断），使每次首连都看起来
    「曾在线」，首连 Provisioning 失败被降级为 warning。现在握手只保留已有 `lastOnlineAt`，由
    `set_connection_state(online)` 在屏障通过后推进；首连失败不写 `lastOnlineAt`，屏障按握手前的值判断。
  - 首连屏障失败返回结构化 `provider-provisioning-failed` / `provider-provisioning-unsupported`
    （加入 `PUBLIC_ESCODE_ERROR_CODES`）。`/api/source-server-resolve-profile` 把该码透传给 Main，
    Renderer 以本地化标题呈现，原始远端错误（如 `source-read-failed`）保留在折叠的 Details 中；
    原始凭据值不进入 UI、事件或日志。
- 后续触发：Personal Config 写入（`ProviderService` 写入成功广播）、`setting.update` 命中
  `providerFamilyDomain` / `providerFamilyConnectionSelections`、`credential.save` / `delete` 命中
  allowlist key。Host 对全部在线 profile 推进 generation；同 profile 同时最多一个同步，在途触发只合并为
  一次尾随同步。迟到结果不改变已发布连接状态。
- 安全：Envelope 与凭据值不写日志、不进事件、不持久化到 Main。warning 只含 trigger、profileId、status、
  errorCode 与远端公开错误文本，不含 Envelope。

S4-g 验收：

1. 首连 target 返回 `applied` → 发布 online；Personal Config、账号窄设置与 allowlist 凭据出现在 Envelope。
2. 首连返回 `failed` / `rollback_failed` / `unsupported`、channel 缺失或 source 读取失败 → 不发布 online，
   正式 `failed` 且带固定 errorCode。
3. 曾在线后的触发同步失败不拆连接，只 warning；下一轮成功触发恢复。
4. 同一 profile 的在途触发合并为一次尾随同步；断线/换代后的迟到结果不影响状态。
5. 非 allowlist 凭据（含坏值）不参与同步；allowlist 空值跳过；坏 Personal 文件整体失败。
6. 真实 SSH 远端完成 Provisioning 后可用本地 Personal Provider 模型。

S4-g 远端 runtime preferences 应答（2026-10-31）：

- 背景：`desktop-attached-remote` 下 Agent 运行在远端，但 app-global 设置权威仍在本地 Host；远端
  Agent 物化会话时通过 `escode-agent.onDynamicSessionRuntimePreferencesRequest` 动态事件请求
  `{requestId, sessionId, scope}`。Rust Host 若不监听，Agent 会在首个 prompt 前等待 15s 后超时。
- Owner：`escode_host_services::source_server_runtime_preferences` 持有监听与应答；本地
  `session/requestRuntimePreferences` 与远端动态事件共用同一份 `preferences` 解析，避免两侧默认值
  漂移。`ConnectionsOptions.preferences_home` 只注入本地 settings 根，不复制设置到连接状态。
- 时序：Initialize 成功且远端广播 `escode-agent` 后注册监听；每次事件读取本地 `settings.json`
  （与本地 Host 同源）并调用 `escode-agent.respondSessionRuntimePreferences`，载荷按 TS
  `ProxyChannel` 契约包成单元素数组 `[{requestId, resolution}]`。resolved 返回
  `preferences`；读取失败返回 `{status:"failed", message}`，由 Agent 编码为 `-32603`。
- 收口：连接 dispose 时事件发送端被丢弃，监听循环退出；断线后不再发送应答。旧远端未广播
  `escode-agent` 时跳过监听，不改变连接状态。
- 安全：事件只含 session/request 标识，不写日志中的设置值；应答不进入 Renderer、Main 或持久化。

S4-g 远端 runtime preferences 验收：

7. 远端动态请求命中本地设置 → Agent 收到 array-wrapped resolved 偏好；读取失败 → failed 且无连接
   状态变化；连接关闭后同一请求不再收到应答。

S4-g 完成（2026-10-31）：首连屏障、后续触发与远端 runtime preferences 应答已通过 host-services /
host-rpc / host-proto 单测、11 例真实 TS fixture 跨语言测试，以及真实 SSH formal E2E
`desktop-e2e-20261031-004508-182`（`1 passing (14.5s)`）。该例在 App 关闭时写入 SSH Personal
Provider，首连后远端 `provisioning.json` 记录增长、`provider_config.json` 含本地 Provider；远端经
SSH 反向转发访问本地 replay provider 并返回首轮回复，全程无 `escode-host-remote-*` 进程。

### 11.8 Host key 策略

S4-h 完成（2026-10-31）：

- SSH transport 不再使用 `StrictHostKeyChecking=no` + `UserKnownHostsFile=/dev/null`；启动参数固定为
  `StrictHostKeyChecking=accept-new` + Desktop 自有 `UserKnownHostsFile`，缺失路径时显式失败而不是静默
  关闭校验。
- known_hosts 固定在 `<dataDir>/.escode/v2/known_hosts`（Main 未交付 `dataBaseDir` 时 SSH 连接显式失败）；
  不写用户全局 `~/.ssh/known_hosts`。系统 ssh_config 若启用 `HashKnownHosts`，条目可能是 `|1|salt|hash`
  形式，校验行为不变。
- 握手 EOF/超时会把 transport 的有界 stderr 带进错误链；`REMOTE HOST IDENTIFICATION HAS CHANGED` /
  `Host key verification failed` 归类为 `remote-host-key-changed`，发布 `failed + retryable=false`，不
  自动覆盖 known_hosts。preflight 先于握手发现同样的错误时也走该分类，不能退化成可自动重试的安装失败。
- `sshConfigAlias` 场景仍显式传 `UserKnownHostsFile`，避免 OpenSSH 配置漂移。
- `BatchMode=yes` 仍用于无密码路径；密码/口令只经 askpass env，不进 argv。
- 验证：`cargo test -p escode-host-services` 681 lib passed（含 accept-new/自定义 known_hosts、
  缺失 known_hosts、仅识别明确 key 变化、stderr 归类、preflight 错误码与 `retryable=false`）；
  `cargo test -p escode-host-rpc -p escode-host-proto`、`cargo clippy ... -- -D warnings`、
  `node scripts/check-escode-host-boundaries.mjs`、`pnpm check:escode-host`、`pnpm typecheck`、
  `pnpm lint`（210 基线下 0 error）、`pnpm architecture:check --changed` 通过。
- 真实 SSH formal E2E `desktop-e2e-20261031-013132-694`（127.0.0.1:22225）：首连后 Desktop known_hosts
  出现目标条目；断开并替换为错误 key 后重连，状态收敛到 `failed`、`errorCode=remote-host-key-changed`、
  `retryable=false`，known_hosts 保持被篡改后的内容，`1 passing (8.8s)`。
- 回归：S4-g 首连 Provider E2E 重跑 `desktop-e2e-20261031-013349-201`，`1 passing (14.1s)`，证明
  accept-new 写入已知 host 后首连仍正常。
- 该策略只影响 Host 发起的 SSH，不改变用户手工终端行为。

### 11.9 验收场景（S4）

1. 空远端 + 显式连接：preflight → 安装 → hello/Initialize → online；`installPreview.sizeBytes > 0`。
2. 远端缺 curl/wget/tar/sha 工具：local-upload 可安装；remote-download 返回 `remote-preflight-failed`。
3. 安装中断或校验失败：旧 release 仍可启动，错误码显式，无 `online` 发布。
4. 兼容矩阵：兼容旧版 → `updateAvailability: available` 仍连接；不兼容旧版 → `needs-upgrade`；
   不兼容新版 → `upgradeTarget: controller` 且不降级。
5. 更新：`force:false` 有运行任务 → `source-server-update-running-tasks`；`force:true` → `updating` →
   新版本 online，`remoteSessionId` 换代。（S4-d 已验收，见 §10.9 落地记录与 `desktop-e2e-20261030-224107-752`。）
6. 更新失败：更新前在线则旧连接保持 online，状态只附加错误与重试入口。（S4-d 已验收：安装/启动失败回滚旧 release 并恢复旧连接，见 §10.9。）
7. 重启：7 天内 `lastOnlineAt` 自动连接（≤3 并发、只试一次）；断线 2/4/8s ×3 后 `offline` + 预算耗尽码。（S4-e 已验收，见 §10.9 落地记录与 `desktop-e2e-20261030-231049-242` / `desktop-e2e-20261030-231303-109`。）
8. 迁移：部分失败按 `failedPathCount` 展示；远端不支持 → `unavailable`，不显示 0 失败。（S4-f 已验收，
   见 §11.6 S4-f 落地设计与落地记录。）
9. Provider：首连下发本地 Personal Provider 与 OAuth 凭据后远端可选本地模型；首连 apply 失败不发布 ready；
   后续失败只 warning。（S4-g 已验收，见 §11.7 落地记录与 `desktop-e2e-20261031-004508-182`。）
10. Host key：首次写入 known_hosts；key 变化返回 `remote-host-key-changed` 且不自动覆盖。（S4-h 已验收，
    见 §11.8 落地记录与 `desktop-e2e-20261031-013132-694`。）
11. 身份变化：远端持久身份与 Profile 已绑定的 `sourceServerId` 不同 → `identity-change-pending` 且零写入；
    用户确认后旧 Profile 退休、新 Profile 绑定新身份并 online。（S4-j 已验收，见 §11.12。）
12. 全程无 per-window Node Host 参与；Node 路径只作为 S5 删除前的对照基线。

### 11.10 切片与退出条件

| 切片 | 内容                                                                           | 退出条件                  |
| ---- | ------------------------------------------------------------------------------ | ------------------------- |
| S4-a | 兼容清单 + hello 扩展 + 状态字段 + operation log 发布                          | 单测 + 兼容矩阵           |
| S4-b | release 解析、preflight、install preview、部署决策（dry-run）                  | 单测 + 真实 SSH preflight |
| S4-c | 首次安装（两种 install mode）+ 部署锁 + component meta                         | 空容器首连 E2E            |
| S4-d | 显式更新 + running-task guard + 失败保留旧 release                             | 更新 E2E（含 force）      |
| S4-e | `lastOnlineAt` 持久化 + 启动自动连接 + 恢复预算（已落地）                      | 重启 / 断线 E2E           |
| S4-f | `source-server-workspace` 转发 + 迁移投影（远端实现另行验收，已落地）          | 契约 + 快照测试           |
| S4-g | Provider provisioning 首连屏障 + 后续触发 + 远端 runtime preferences（已落地） | 真实 SSH 模型可用 E2E     |
| S4-h | known_hosts 策略（已落地）                                                     | 单测 + 真实 SSH           |
| S4-i | Windows / macOS 部署与恢复验证（Windows 交叉编译已过；桌面 E2E 待跑）          | 三平台 E2E                |
| S4-j | 握手身份记录 + `identity-change-pending` + Profile 重绑定（已落地）            | 单测 + RPC/集成           |

S4 完成判据：上述 11 条验收在 Linux 全绿；Windows/macOS 至少完成安装、更新、恢复三条主链。S4 期间 Node
Host 远程路径仍可作为对照，但 Rust 路径不得回退 Node。

S4-d 完成（2026-10-31）：显式更新、running-task guard 与失败回滚已通过 9 个纯单测 + 7 个连接/flow 测试，
以及真实 SSH formal E2E `desktop-e2e-20261030-224107-752`（首装后 `force:true` 更新，`1 passing (5m 14.7s)`）。
`force:false` 时运行任务清单不可读也按 guard 拒绝；失败只附加错误、不把 online 改判为 `needs-upgrade`。

S4-e 完成（2026-10-31）：`lastOnlineAt` 由 Host 时钟推进并经 settings 串行写队列落盘；迟到/较小时间戳不覆盖
更新握手，已删除 Source 不重建，离线重试不推进时间。启动自动连接只对近 7 天在线过的 Source 各尝试一次
（≤3 并发、单次 10s、`ConnectIntent::Recover` 不安装/不升级/不弹凭据），并复用 registry 状态与 operation log。
online 链路断开后发布 `retrying` attempt 1→3（每档 2s/4s/8s 延迟前先发布），预算耗尽发布 `offline` +
`remote-recovery-budget-exhausted` + `recoveryAttempt=3`/`recoveryLimit=3`/`recoveryBudgetExhausted=true`；
手动断开推进代次取消待执行恢复且不伪造计数。Host 启动只负责「近 7 天 Source」候选，Renderer 恢复断连 tab 后
消费 Host `online` 事实换发 `remoteSessionId`；Host 不读取窗口 tab 状态，超出 7 天窗口的 Source 保留手动重连入口。
验证：`cargo test -p escode-host-services` 655 lib + 3 + 1 integration 通过（新增 5 个恢复测试、4 个候选/时间戳
测试、2 个 settings 并发/单调测试、1 个恢复字段序列化测试）；`cargo test -p escode-host-rpc -p escode-host-proto`、`cargo clippy ... -D warnings`、
`node scripts/check-escode-host-boundaries.mjs`、`pnpm check:escode-host`、`pnpm typecheck`、`pnpm lint`（210 基线
0 error）、`pnpm architecture:check --changed` 通过。真实 SSH formal E2E：App 重启后按 `lastOnlineAt` 自动恢复
（`desktop-e2e-20261030-231049-242`，`1 passing (14.8s)`）与真实断开 SSH transport 后 `retrying` → 自动 `online`
并续发历史（`desktop-e2e-20261030-231303-109`，`1 passing (23.3s)`），两者均确认无 per-window Node Host。

S4-f 完成（2026-10-31）：迁移统计 owner 落在 `SourceServerConnections` 的运行态投影（不持久化），以
`(profileId, attempt)` 作为物理连接 token；`getSnapshot` 同步返回 directory/states 并合并
`requires-connection` / `loading` / `available` / `unavailable`，顶层总数只累加 available。统计读取异步补全，
完成时通过目录事件提升 registry generation；连接换代、Source 移除与 dispose 后的迟到结果直接丢弃。
`refreshMigrationCounts: true` 使完成项失效、在途项合并一次重读；默认 false 只读现有投影。管理面新增
`getMigrationReport` / `exportMigrationReport`（缩进 JSON + 换行）、`abandonWorkspaceMigration` /
`retryWorkspaceMigration`（转调远端并在 finally 语义下失效统计、发布目录事件）；未连接返回
“Source Server must be connected”。远端广播的 `source-server-workspace` 继续由既有 scoped 转发原样暴露。
验证：`cargo test -p escode-host-services` 662 lib + 3 + 1 integration、`cargo test -p escode-host-rpc -p escode-host-proto`、
跨语言 fixture `source_server_client_ts` 6 例（新增 loading→available、报告/放弃/重试、断线 requires-connection）、
`cargo clippy --all-targets -D warnings`、`node scripts/check-escode-host-boundaries.mjs`、`pnpm check:escode-host`、
`pnpm typecheck`、`pnpm lint`（210 基线 warning / 0 error）、`pnpm architecture:check --changed` 全部通过。

### 11.11 S4-i：Windows / macOS 部署与恢复验证

> 记录：2026-10-31。当前开发机只有 Linux x64，没有 Windows/macOS 桌面环境，也没有从这些平台可达的
> SSH 目标。本节只交付「可复现的编译级证据 + 平台审计 + 待执行 runbook」，**S4-i 尚未验收**。

已完成（可在 Linux 上复现）：

- Windows 编译矩阵：安装 `mingw-w64` 并 `rustup target add x86_64-pc-windows-gnu` 后，
  `cargo check --locked --target x86_64-pc-windows-gnu --workspace --all-targets` 与
  `cargo clippy --locked --target x86_64-pc-windows-gnu --workspace --all-targets -- -D warnings`
  均通过（含 test target）。
- 编译检查抓到并修复 2 个只在 Windows 触发的 clippy 失败：
  `source_server_bootstrap_test_support::write` 与 `source_server_install_tests::write` 的
  `executable` 参数在非 unix 下未使用。CI `checks` 三平台以 `-D warnings` 运行，这两个 warning
  会直接变红。
- POSIX shell fixture 的平台边界：`source_server_exec_tests`、`source_server_install_tests`、
  `source_server_connections_update_tests` 整体 `#![cfg(unix)]`；deploy-lock / recovery /
  connections-install 只给依赖本机 `sh` 的用例加 `#[cfg(unix)]`，保留纯逻辑用例在 Windows 运行
  （部署锁脚本生成、bootstrap 安装准备、preflight host-key 分类、启动自动连接并发上限）。
  这样 Windows 上的 `pnpm test:escode-host` 不再依赖 Git Bash，安装/更新/恢复行为由桌面 E2E 验证。
- Windows 测试目标可链接：`cargo test --target x86_64-pc-windows-gnu --workspace --no-run` 链接
  host-services / host-rpc / host-runtime / host-surface 的全部 lib 与集成测试可执行文件；Linux
  套件保持 682 lib + 3 + 1。SSH 集成测试在无 `ESCODE_E2E_SSH_HOST` 时跳过，默认 askpass 路径按
  Windows 追加 `.exe`（`scripts/test-escode-host-interop.mjs` 已同样处理）。
- CI 同款命令 `pnpm test:escode-host` 在 Linux 通过：workspace 全量单测 +
  `scripts/test-escode-host-interop.mjs`（Node/Rust 交替写同一 settings 文件 2 例）。这同时是
  Windows/macOS CI 将执行的入口。
- 2026-11-01 复查：`x86_64-pc-windows-gnu` 目标的 `cargo clippy --workspace --all-targets -- -D warnings`
  在 CLI 与 Host 两个 workspace 重新通过；复查发现并修复两处新的 Windows 专属失败
  （`source_server_bootstrap_test_support` 的 `#[cfg(unix)]` 导入、`file_stamp` 测试的
  `atomic_replace` 辅助）。同一轮把 CLI 门禁的 fmt 补成 `--all`，避免 crate 格式问题只在 Host 门禁暴露。
- 新增 `preserves_known_hosts_paths_for_macos_and_windows_data_dirs`：macOS 默认数据目录含空格
  （`~/Library/Application Support`）、Windows 路径含反斜杠，`-o UserKnownHostsFile=` 必须原样
  透传。已在本机 OpenSSH 9.6 实测 `-o` 取值不按空白拆分，因此不加引号、不做转义；若后续改动引入
  引号或转义，该测试会失败。
- `source-server-rust-transport-drop.test.ts` 增加 `process.platform !== "linux"` 跳过：该用例依赖
  `/proc` 定位 Rust Host 的 `ssh` 子进程并 `SIGKILL`，Windows/macOS 没有对应实现；跨平台主链改由
  install / restart-recovery 用例覆盖。
- 2026-11-02 Windows 客户端 POSIX 路径修复：Windows SSH E2E（run `41340564228`，job
  `124950642203`）已接通 WSL2 sshd 并完成部署，但目录选择器停在「初始目录未就绪」。
  根因是 `host-rpc` 的 `project.readDirectory` / `saveProject` 用 `std::path` 判断和归一
  远端 owner 的路径：Windows 下 `Path::new("/root").is_absolute()` 为 false，且
  `normalize_path` 以 `\` 拼接，`/root` 会变成 `\root`。远端 owner 由远端 Linux/macOS
  解释，必须用 POSIX 规则；新增 `is_posix_absolute` / `posix_normalize` /
  `normalize_owner_path`，控制端固定传 `posix=true`，本机 owner 仍沿用平台语义，
  并加 `owner_path_uses_posix_rules_for_remote_owners` 锁定回归。
  修复后实跑 run `41343996871`（job `124961277623`）：`SSH-P0-PROJECT-01` 1 passed
  （2m59s），该 run 的 8 个作业全部 green（含三平台 curated 矩阵、Linux/macOS/Windows
  SSH、musl runtime 与 app perf）。同轮 Linux SSH 作业 `124960188757`、macOS SSH 作业
  `124960188825` 亦 green。Windows 侧该轮仍为单例首连；多窗口/重启恢复用例
  尚未在 Windows/macOS 执行。

平台审计结论（静态）：

- 生产路径没有 unix 专属 API：`source_server_*.rs` 中只有 `source_server_archive::mode` 与测试
  fixture 使用 `#[cfg(unix)]`；安装脚本、部署锁与 preflight 都在远端 POSIX 目标上执行。
- 远程资产按目标平台打包：Windows x64 桌面包内置 `linux-x64` release，macOS arm64 内置
  `linux-arm64`（`release.yml` 的 `ESCODE_REMOTE_ASSET_PLATFORMS`），跨平台首连的资产集完整。
- macOS 编译检查在本机不可行：`rusqlite` bundled 需要目标平台的 C 工具链/SDK，
  `aarch64-apple-darwin` 无法交叉编译。macOS 证据只能来自真实机器或 CI。

未完成（S4 退出条件仍不满足）：

- 2026-11-01：三平台桌面矩阵新增 `source-server-rust-server-direct.test.ts`（worker 内拉起真实
  `escode-server`，走 Rust Host 的 HTTP/WS 远程链路）。它为 Windows/macOS 提供 server transport 的
  主链证据（首连、App 重启恢复、断线原位重连、双窗口共享、零 Node 远程进程），fixture 用稳定端口
  代理隔离后端重启，避免 Windows 同端口 TIME_WAIT 造成的测试环境差异。该用例**不替代**下面三条
  SSH 主链：SSH 链路需要对应平台可达的目标机，仍待补。
  本机 Linux x64 复跑：单跑 `desktop-e2e-20261101-005906-288`（1 passing / 18.6s），按 CI 默认
  四条清单合跑 `desktop-e2e-20261101-010134-706`（4 passing / 1m08s）；Windows/macOS 的 CI 实跑
  结果在 push 后登记。
- 2026-11-01 首次三平台 CI 实跑（`desktop-e2e-matrix` run `41260831810`，commit `73cbfb1e`）：
  ubuntu 通过；windows 在 fixture 启动 `escode-server` 时崩溃（libuv `fs-event.c` 断言
  `_wcsnicmp(filename, dir, dirlen)`，exit `3221226505`），macOS 报裸 `Timeout`（阶段不可定位）。
  两个平台都没拿到该用例的通过证据，S4-i 保持未完成。
- Windows 崩溃根因（已修）：fixture 用 `tmpdir()` 建 workspace / dataDir，Windows 路径含 8.3 短名
  （`C:\Users\RUNNER~1\...`），server 对 workspace 做递归 `fs.watch`，libuv 用短名目录与 OS 回报的
  长名前缀比较不一致即断言退出。`local-escode-server.ts` 改为先 `realpath(tmpdir())` 再拼子目录，
  同时不受 macOS `\var` → `\private\var` 符号链接影响；这是测试环境路径，不是产品缺陷。
- 2026-11-01 第二轮 CI（run `41262565326`，commit `e4baa971`）：realpath 修复生效，libuv 崩溃消失；
  macOS 该用例通过，ubuntu 通过；windows 在 `select-directory` 阶段失败——远端目录选择辅助
  （`source-server-rust-remote.ts`）只按 `/` 切父目录，Windows 目标路径 `C:\...` 的
  `lastIndexOf("/")` 为 -1，父路径退化成整条路径去掉末字符，永远找不到目标目录。已改为同时接受
  反斜杠与 `/` 分隔符并保留盘符根 `C:\`；本机 Linux 复跑 `desktop-e2e-20261101-021944-470`（1 passing / 18.9s）。
  windows 通过结果待第三轮 CI 确认。
- 2026-11-01 第三轮 CI（run `41264933484`，commit `c79b6a7b`）：windows 与 macOS 的该用例**首次通过**，
  说明 libuv 短路径与目录分隔符两处修复成立；ubuntu 该用例失败（`2m 2s`，裸 `Timeout`，无阶段信息），
  同 commit 本机 Linux 连续复跑通过（最近一次 `1 passing / 18.7s`）。同一用例在不同平台上分别通过、
  ubuntu 偶发超时，按 flaky 处理，不据此判定产品回归。
- 2026-11-01 第四轮 CI（run `41266592976`，commit `dbe4bb91`）：windows、ubuntu 通过，macOS 失败。
  阶段日志显示最后停留在 `restart-recovery`：`reloadElectronSessionSafely` 里的 `reloadSession()` 重建
  WebDriver session 时超过 30s 默认 `connectionRetryTimeout`，WDIO 抛裸 `Timeout`。三次推送里
  ubuntu/macOS 各命中一次、彼此不重叠，按同一根因（慢 runner + session 重建）处理。矩阵 job 现设
  `ESCODE_E2E_WEBDRIVER_REQUEST_TIMEOUT_MS=120000`（仍保留挂起边界），第五轮验证。
- 2026-11-01 第五轮 CI（run `41268279642`，commit `939350f8`）：**三平台桌面矩阵全绿**——
  ubuntu / windows / macos 的 `source-server-rust-server-direct.test.ts` 全部通过，覆盖真实
  `escode-server` 首连、App 重启恢复、断线原位重连、双窗口共享与零 `escode-host-remote-*`。
  这是 server transport 的跨平台主链证据；SSH 三条主链（首装/更新/重启恢复）仍缺对应平台可达的
  Linux 目标机，S4-i 不因本条转绿而完成。
- 2026-11-01 复跑（run `41269283544`，commit `f2598803`）：同一用例三平台再次全绿——ubuntu / macos /
  windows 的 `desktop e2e` 均 success；`app perf` 因后续推送触发并发取消（cancelled，非失败）。
- 2026-11-01 `pnpm test:escode-host` 首次真正跑到 Windows/macOS（此前分片被 CLI 用例 fail-fast 或取消），
  暴露并修复三处**夹具**缺陷（产品路径未改）：
  1. `source_server_bootstrap_test_support::probe_stdout` 恒发 `kernel_ostype=Linux`，macOS 上
     `Darwin` + `Linux` 被 `resolve_remote_platform` 判成 `linux-*`，于是去找
     `manifest-linux-arm64.json`。改为按宿主平台发 `Darwin`/`Linux`。
  2. `write_release` 没写 `node-pty/{platform}/spawn-helper`，而 `component_install` 在 `darwin-*`
     上要求它，macOS 的 local-upload 用例全部落到「local release artifacts are incomplete」。
     夹具补齐该 darwin 专属条目。
  3. `source_server_identity_tests` 用 `temp.path()` 字面值比对 `canonicalServerRoot`；macOS 的
     `tempfile` 在 `/var/folders/...`（`/private/var` 符号链接），远端脚本按 TS
     `realpath(layout.serverRoot)` 写入物理路径。字段名即语义，测试改为比对 realpath。
- 2026-11-01 Windows 专属：`source_server_client_ts` / `source_server_remote_scope_ts` 的
  `workspace_root()` 裸 `canonicalize()` 在 Windows 回 `\\?\D:\...`（verbatim），把它当脚本参数交给
  `node` 会被 `Module._findPath` 解析成 `D:`（`EISDIR`），整条跨语言对拍在 windows-latest 全红。
  夹具按 `rust-windows-parity.md` 的规则去掉非 UNC 的 verbatim 前缀。
- 2026-11-01 跨平台 CI 转绿（run `41282629407`，commit `f6f4b10f`）：`checks`（含 Windows 目标
  clippy `-D warnings`）、`bench`、`test` 的 9 个分片（ubuntu/macOS/windows × 1..3）与
  `desktop e2e matrix` 三平台全绿。其中 `test (windows-latest, 1/3)` 与
  `test (macos-latest, 1/3)` 是 `pnpm test:escode-host` 首次真正跑到这两个平台：
  Windows 侧 15 个跨语言对拍用例（10 client + 5 remote scope）与 macOS 侧 10 个
  bootstrap/identity 用例 + V4 帧差分全部通过。修复项见上面三条夹具缺陷，以及 §10.11 的
  EventListen eager 入队（macOS 的 `v4_frames_match_the_node_remote_oracle` 5s 无帧超时）。
  同轮 `test (windows-latest, 2/3)` 的两个 CLI 差分套件（`attachment-limits`、
  `read-text-parity`）在补 Windows 录制 golden 后通过。
  **这不改变 S4-i 结论**：SSH 三条主链（首装/更新/重启恢复）仍需 Windows/macOS 可达的 Linux
  目标机，CI 的 SSH job 固定 `ubuntu-latest`。
- 2026-11-01 复发的 e2e flake（已定位，用例侧修）：`source-server-rust-server-direct` 在
  `restart-recovery` 阶段以裸 `Timeout` 失败（run `41282629303` 首次、`41285070206` 再次；
  两次都只有 ubuntu/macOS 命中，windows 通过；同 commit rerun 通过）。
  **早前“ChromeDriver 随 App 退出被回收”的判断不成立**：日志里 deleteSession 的 body 是
  `{"deleteSessionOpts":{"shutdownDriver":false}}`，说明 `reloadElectronSessionSafely` 的参数形态
  正确（WebdriverIO 只从 `body.deleteSessionOpts.shutdownDriver` 取值），delete 时没有
  `Kill driver process`。`Kill driver process with PID …`、`invalid session id` 与
  `/json/version` 的 `fetch failed` 都发生在 120s 超时**之后**的 teardown，是失败的结果而不是原因。
  真实链路：重启后的新实例只建了 `__base__ …/ESCodeProject` 连接，远端 workspace 连接始终没出现，
  于是 `waitForWorkspaceApp` 一直轮询到超时；用例正是在 `waitForRemoteSession` 返回后立刻退出。
  触发条件是 tab 持久化写回的 300ms 防抖（`useTabPersistence` 的 `DEBOUNCE_MS`）：三次日志实测
  「远端连接建立 → App 退出」分别为 267ms（失败 macOS）、349ms（失败 ubuntu）、387ms（通过
  ubuntu）。丢写后 `setting.json.lastActiveWorkspace` 仍是上一条本地 tab，重启就只恢复本地
  workspace。修复：重启前先在用例里调用
  `waitForPersistedActiveRemoteWorkspace(CASE_MARKER, workspaceIdentity)`，与 SSH
  `restart-recovery` 用例同一守卫；重启恢复的前提是设置已落盘，不是“切到远端后立刻退出”。
  验证（run `41288521726`，commit `3f804de1`）：三平台 `desktop e2e` 全绿，ubuntu 日志里
  `stage=restart-recovery` 到 `deleteSession` 之间为 524ms，即守卫补上了原本被截断的写回窗口。
  产品侧同样存在这个窗口（切到远端后 <300ms 退出会回退到上一条 workspace）；本轮只把
  “已落盘”作为用例前置条件，未改 quit 前的持久化语义。
- 诊断补强：阶段标记同时打到 worker 日志（`[SOURCE_SERVER_RUST_SERVER_DIRECT] stage=<name>`），
  下一轮 ubuntu 失败可直接定位卡住的阶段。
- 排查能力：用例新增阶段标记（`seed-profile` / `open-profile` / `select-directory` /
  `wait-workspace-ready` / `wait-remote-session` / `restart-recovery` / `disconnect-reconnect` /
  `second-window` / `assert-zero-node-host`），失败时错误信息带阶段名；`after` 钩子改为 `server?.stop()`，
  避免 `before` 失败时 `reading 'stop'` 掩盖真实原因。macOS 的 `Timeout` 需下一轮 CI 用阶段名确认。
- 2026-11-01 删除 Node 远程路径（S5-f-5）后，Linux x64 上按本 runbook 复跑的三条主链均已通过：
  - 首次安装 + 显式更新：`source-server-rust-install.test.ts`，
    `desktop-e2e-20261101-000342-581`（`1 passing (5m 22s)`；空目标 `needs-install` →
    `installPreview.sizeBytes > 0` → 远端 `~/.escode/server/node` 与 `zcode-server.cjs` →
    `force` 更新到 `3.15.2` 后仍 `online`）。首跑时目标机残留上次安装，用例按「空目标」契约
    先 `rm -rf /root/.escode` 才能复现 `needs-install`，这是 fixture 前置条件而非产品缺陷。
  - 重启恢复：`source-server-rust-restart-recovery.test.ts` 于
    `desktop-e2e-20261031-235726-737` 通过（App 重启后按 `lastOnlineAt` 自动恢复，零 Node Host）。
  - host key：`source-server-rust-host-key.test.ts` 于 `desktop-e2e-20261101-000052-414` 通过
    （首连写 Desktop 自有 known_hosts；key 变化显式 `remote-host-key-changed` 且不覆盖）。
- Windows/macOS 上三条主链的真实 E2E：首次安装、显式更新、重启恢复。
- 现状缺口：本机无 Windows/macOS 桌面环境；CI 的 SSH E2E job 固定 `ubuntu-latest`（Docker sshd），
  三平台 e2e matrix 不含 SSH 用例；且 S3/S4 源码尚未提交，已有 CI 记录（`01ebe258`，2026-10-30）
  不覆盖任何 `source_server` 代码。
- 因此不得把 S4 标记为完成，也不得据此执行 S5 删除。

待执行 runbook（Windows PowerShell；macOS 只替换环境变量语法）：

```powershell
pnpm install --frozen-lockfile
node scripts/build-desktop-agent-cli.mjs
cargo build --locked --manifest-path apps/escode-host/Cargo.toml

# 目标必须是 Desktop 内置资产对应的架构：Windows x64 / macOS arm64 -> linux-x64 / linux-arm64。
$env:ESCODE_REMOTE_ASSET_PLATFORMS = "linux-x64"
$env:ESCODE_E2E_SSH_HOST = "<linux-host>"
$env:ESCODE_E2E_SSH_PORT = "22"
$env:ESCODE_E2E_SSH_USERNAME = "root"
$env:ESCODE_E2E_SSH_PASSWORD = "<password>"
$env:ESCODE_E2E_SSH_WORKSPACE_PATH = "/root/workspace"
# restart-recovery 需要两个不同的绝对路径，且必须在目标机预先存在。
$env:ESCODE_E2E_SSH_WORKSPACE_PATHS = "/root/workspace,/root/workspace-two"

pnpm --filter @escode/desktop exec wdio run wdio.conf.ts --spec ./test/e2e/conversation-session/source-server-rust-install.test.ts
pnpm --filter @escode/desktop exec wdio run wdio.conf.ts --spec ./test/e2e/conversation-session/source-server-rust-restart-recovery.test.ts
pnpm --filter @escode/desktop exec wdio run wdio.conf.ts --spec ./test/e2e/source-server-rust-host-key.test.ts
```

判定标准：install 用例断言首装后 `online` 且 `installPreview.sizeBytes > 0`，随后 `force:true`
更新到 `ESCODE_E2E_SSH_APP_VERSION`（默认 `3.15.2`）；restart-recovery 断言 App 重启后按
`lastOnlineAt` 自动恢复；host-key 用例断言 known_hosts 写入与 key 变化后
`errorCode=remote-host-key-changed`、`retryable=false`。三条主链的证据需包含 wdio 报告与
`.e2e-artifacts` 产物。

### 11.12 S4-j：握手身份记录与 Profile 重绑定（2026-10-31 落地）

> 设计记录：2026-10-31。目标是把远端持久身份从“只存在于 Node 路径”搬到 Rust Host：Host 记录
> `sourceServerId` / `osUser` / `hostname` / 声明 workspace，并在身份变化时只通过用户确认重绑定。

身份来源与 probe：

- `sourceServerId` 以远端 `~/.escode/server/install.json` 的 `installationId` 为准，schema 与
  `escode-server-cli` 的 ownership marker 相同（`product=escode-server`、`schemaVersion=1`、
  `canonicalServerRoot`、`installationId`、`installedAt`）。
- 文件不存在时 Host 生成 UUID 并在同一 POSIX 脚本内以“临时文件 + hard link”原子创建；已存在时只读，
  不覆盖。`canonicalServerRoot` 由 `cd <root> && pwd -P` 取得；值经 JSON 转义。
- 同一脚本返回 `osUser`、`hostname` 与默认 workspace（`ESCODE_SERVER_WORKSPACE` 未设置时取脚本 cwd），
  字段以 NUL 分隔，避免路径中的换行/分隔符破坏解析。不得从 Profile、SSH host 或路径推断身份。
- probe 顺序：transport 解析 → 显式安装（`ConnectIntent::Explicit`）→ probe → 启动 server/Initialize。
  身份变化且未确认时 **不启动 server、不写 settings**；`Recover` 不安装但仍 probe。

记录语义（`SourceServerRegistry::record_handshake`，与 Node `recordHandshake` 对等）：

```text
previousSourceServerId 存在且 != handshake.sourceServerId
  ├─ confirmIdentityChange != true → 零写入，返回 identityChanged=true
  └─ confirmIdentityChange == true → commitIdentityReplacement
否则 → 原位更新 Profile + upsert Source + 重写 history 身份
```

- 未确认：连接管理器发布 `identity-change-pending`，携带 `pendingSourceServerId`、
  `errorCode=source-server-identity-change-pending`、`retryable=false`；RPC 错误同时透传
  `code=source-server-identity-change-pending`。已有连接/Provider 状态按收口路径释放。
- 确认重绑定：生成新 `profileId`；SSH 凭据按新 key（`connection-profile:<newId>:password` /
  `:private-key-passphrase`）复制，失败回滚并显式失败；旧 Profile 标
  `lifecycleState=retired` / `retiredReason=source-reinstalled`，新 Profile 为 active 并绑定新
  `sourceServerId`；旧 Source 标 `installState=uninstalled` 且重选 preferred，新 Source upsert。
  `sourceServerWorkspaceHistory` 保持旧身份，不改写到新设备。
- 未变化：Profile 写入 `sourceServerId`、`lastConnectedAt`、清 `lastConnectError`、`lifecycleState=active`；
  Source 保留已有 `displayName`，首次用 `sshConfigAlias || "<hostname> · <osUser>" || sourceServerId`；
  写 `osUser` / `hostname` / `lastSeenVersion` / `lastOnlineAt` / `preferredProfileId` / `installState`。
- history：仅该 `profileId` 的条目重写 `sourceServerId` 与 stable identity
  `remote:server:<sourceServerId>:<normalizedPath>`，保留 `legacyWorkspaceIdentity`。Node 的
  `migrateControllerIdentity`（控制器行改名）尚未在 Rust 实现，本切片不调用、不伪造
  `migrationFailures`；legacy 条目由 §11.6 的迁移统计/报告暴露。
- `declaredWorkspaces` 使用设备级 canonical identity：远端回传 identity 的设备/路径不匹配时覆盖为
  canonical，仅记 debug；按 identity 去重，最多 20 条。

连接层：

- 记录成功后使用 **effective profile**（确认场景为新 `profileId`）做 Provider provisioning、连接槽位、
  online 状态与 operation log；旧 Profile 不再发布 online。
- `confirmIdentityChange(profileId)` 调用带确认的连接入口；普通 `connectProfile` 不携带确认。
- E2E 预置 Profile 时不得伪造 `sourceServerId`：夹具经 `ssh2` 读取远端 `install.json`，
  缺失时按同一 ownership marker 契约原子创建，再用真实 `installationId` 生成 workspace identity。
- 写入与 `§9.8` 共用同一 registry 写锁和 `settings::apply_patch`；失败不改内存、不递增 generation。

验收场景：

1. 首次连接：远端无 `install.json` → 原子创建，第二次 probe 读到同一 id；Source 记录出现 osUser/hostname。
2. 身份不变：重连只更新 `lastConnectedAt`/`lastOnlineAt`，不新增 Profile、不改写其它设备 Source。
3. 身份变化未确认：状态 `identity-change-pending`、`pendingSourceServerId` 正确，settings 文件字节不变。
4. 身份变化已确认：旧 Profile retired、新 Profile active、SSH 凭据可用新 key 读取、旧 history 不被改写。
5. 凭据复制失败：settings 不提交、旧 Profile 仍 active、新 key 不残留。
6. history 重写：同 Profile 条目的 stable identity 使用新 `sourceServerId`；其它 Profile/设备不变。

## 12. S5 设计：删除 per-window Node Host 远程路径

> 设计记录：2026-10-31。S5 只在 S2–S4 退出条件全部满足后执行。目标是远程 workspace 只有一条 Host 路径，
> 禁止静默回退；删除的是「per-window Node Host 远程路径」，不是所有 Node 代码。

### 12.1 删除范围

| 删除项                                                     | 位置                                                                                                                                            | 前置                                                 |
| ---------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------- |
| 远端 workspace 懒创建 Node Host                            | `packages/desktop/src/main/desktopRemoteSessions.ts` 的 `node-host` route / `ConnectRemoteWorkspace` / `DisposeRemoteWorkspaceSession` 远端分支 | S5-a                                                 |
| `ESCODE_RUST_HOST=0` 远程回退                              | Main / Host 启动选择与 `rustHostBridge` 远程分支                                                                                                | S5-b                                                 |
| Renderer MessagePort → Node UtilityProcess 远端 attachment | `desktopRemoteSessions` 与 renderer remote session services                                                                                     | S5-c                                                 |
| Host `ProviderProvisioningExecute` 远端分支                | `packages/desktop/src/host/index.ts`                                                                                                            | S4-g                                                 |
| Node remote deploy / preflight / install                   | `packages/server/src/remote/*`                                                                                                                  | 2026-11-01 已删除；保留 backend 原语与构建期资产工具 |

保留：

- `packages/server/src/http.ts` 的 `/api/connect-remote` 与 `/ws/remote/:id` 已于 2026-11-01 退役：
  `packages/web` 在 2026-10-20 随手机远控删除后，仓库内已无调用方；两条 route 删除并由 404 测试锁定。
- `packages/server/src/remote` 中仍被 Rust Host 首装资产、`kind: "server"` 直连和 Adapter 底座使用的模块
  （backend 原语、manifest/归档工具）继续保留，不按目录名整体删除。
- 删除前必须用 `pnpm dep:refs --list-exports` / `pnpm knip` 证明无引用，不能按目录名推断。

### 12.2 直接首连路由到 Rust Host

- 无 `profileId` 的 ssh/wsl/docker 首连（连接向导或历史恢复）必须先由 Rust Host 建立/复用 Connection
  Profile：
  1. Main 按 target 生成或匹配 profile（`sourceServerId` 未知时先连接后回填）；
  2. 同一 target 的并发首连 single-flight；
  3. 申请远端 capability → 走 Rust Host；
  4. 只有 target 明确是 Web 服务端（`kind: "server"`）时才保留 Node/HTTP 路径。
- 不得因为 target 快照缺少 Profile 归属而回退 Node Host；缺字段时由 Main 补齐后再路由。
- `ESCODE_RUST_HOST=0` 只影响本地 scope；远程不再读取该开关。测试禁用 Rust 时改用显式注入 Host 实现，
  不使用环境回退。

**S5-a 实现记录（2026-10-31）**

- Rust Host 新增 `POST /api/source-server-resolve-profile`（进程密钥 + loopback/origin 门禁）。请求为
  `{profileId?}` 或 `{target}`；Host 校验已有 Profile、或调用 `SourceServerRegistry::ensure_profile`
  按 transport + 目标身份匹配/创建 Profile，然后走同一个 `SourceServerConnections::connect` single-flight。
  返回 `{profileId, sourceServerId, target}`；身份变化继续返回 409 + `source-server-identity-change-pending`。
- `ensure_profile` 只在 registry 写锁内持久化 `connectionProfiles`；SSH 明文密码/口令只写入
  `CredentialService`，settings 仅保存 `connection-profile:<profileId>:password` /
  `:private-key-passphrase` 引用。复用 Profile 时保留已有 `sourceServerId`、source 绑定与凭据引用。
- Main 对 ssh/wsl/docker 一律调用该 resolver；按 `profile:<id>` 或与 Rust 逐字节一致的 target 身份键做
  request-level single-flight。成功后把返回的 `profileId` / `sourceServerId` 覆盖到脱敏 target，再申请
  scoped capability；缺 resolver 时返回 `REMOTE_RUST_HOST_UNAVAILABLE`，绝不回退 Node Host。
- 覆盖：`source_server_profile_ensure_tests.rs` 5 例、`rpc_remote_scope.rs` 6 例、`rustHostRemoteScope.test.ts`
  与 `desktopRemoteSessionsRustHostMode.test.ts` 共 24 例；真实 SSH 场景登记在
  `source-server-rust-first-connect.test.ts`（无 `profileId`，验证 settings 回填、凭据脱敏、online 与零
  per-window remote Host）。当前执行环境没有 `ESCODE_E2E_SSH_*`，该 E2E 尚未实跑。

**S5-b 实现记录（2026-10-31）：远程路由与 `ESCODE_RUST_HOST` 解耦**

- `ESCODE_RUST_HOST` 收窄为**仅本地 window Host** 开关：Main 不再在远程路由处读取它，远程 manager 始终
  使用 `remoteWindowHostProcessMap` 与 `ensureRemoteWindowHost`。`=0/false` 不再让远程连接回退 Node Host。
- Node Host 远程所有权被显式收口，而不是靠「没有调用方」维持：
  - `handleConnected` 收到非 `kind: "server"` 的 descriptor 时，删除 pending 请求、向子进程发送
    `DisposeRemoteWorkspaceSession`，并以 `REMOTE_RUST_HOST_REQUIRED` 拒绝；
  - `attachRemoteWorkspaceSessionHost` 只接受 `owner: "node-host"` 且 `target.kind === "server"`，
    其余一律 `REMOTE_RUST_HOST_REQUIRED`。
- 回归：`desktopRemoteSessionsRustHostMode.test.ts` 构造一个绕过路由的 SSH 远端 descriptor，断言被拒绝并
  dispose；该文件与 `rustHostRemoteScope.test.ts` 合计 25 例通过。
- 边界：`kind: "server"`（Web 服务端目标）仍保留 per-window Node Host + HTTP 路径；本步骤只禁止 SSH/WSL/
  Docker 落入 Node Host，不删除 `ConnectRemoteWorkspace` / `DisposeRemoteWorkspaceSession` 消息。

**S5-a renderer session Source identity（2026-10-31）**

- 触发：`SSH-P0-PROJECT-01` 真实 SSH 首连后日志为
  `[Root] device addition delivered { kind: "connected", sourceServerId: undefined }`，
  flow 读取 `sessionsById[sessionId].sourceServerId` 为空，落回
  `allowUnidentifiedDeviceManagement` 的普通目录步骤，向导不关闭。
- 根因：Main 的 route descriptor 已从 resolver 拿到权威 `sourceServerId`，但 `ScopedServicePort`
  只投递 `attachmentId` / `sessionId` / `target`；Preload 与 Renderer parser 丢弃其余 scope，
  session store 也从未收到 `workspacePath` / `workspaceIdentity`（项目打开按 identity 查找
  attachment 时同样失败）。
- 契约（§10.12）：Main 在每次 attachment（connect 与目录 bind 换代）投递
  `sourceServerId?` / `workspacePath` / `workspaceIdentity`；Preload 用
  `scopedServicePortPayloadSchema` 校验后原样转发；Renderer 注册 session 时写入不可变快照。
  Main 是唯一事实源，Renderer 不按 target 或 identity 字符串二次推断。
- 回归：`remoteWorkspaceServicePortBridge.test.ts`（解析 scope / 拒绝缺字段）、
  `rustHostRemoteScope.test.ts`（connect 与 bind 换代都投递权威 scope），真实 SSH
  `conversation-session-ssh-remote-project-create.test.ts`（向导按 `device-management` 关闭并创建项目）。

**S5-a 首连取消契约（2026-10-31）：cancel 必须在协议层中止 SSH**

触发：用户在「连接中」取消（Escape + 确认）。取消语义按 **caller 作用域**，不得按 profile 无差别 abort，
否则会误伤共享同一 Profile 的其它窗口。

- Main：
  1. 进入 resolver 前先登记 per-caller 的 pending（窗口 + renderer `requestId`），与 `pendingByRequestKey`
     同键；取消事件因此能在 resolve 阶段命中，而不是等 `pendingByRequestKey` 注册（原来只在连上后注册）。
  2. single-flight 仍按 transport 身份键共享，但每个 caller 单独登记；取消只移除自己的等待者。
  3. 还有其它 caller 在等同一 flight 时保留 Host 连接；**最后一个 caller** 取消时，`AbortController`
     中止本地 fetch，并 `POST /api/source-server-cancel-profile` 通知 Rust Host。
  4. 被取消的 `createRemoteWorkspaceSession` 以 `remote-connection-canceled` 拒绝；single-flight 表同步移除
     该 flight，后续重试立即建立新的 Host 请求（不复用已取消的 promise）。
- Rust Host：
  1. `POST /api/source-server-resolve-profile` 接受可选 `requestId`（caller key）。带 key 时由
     `SourceServerConnections` 登记 `callerKey → watch::Sender<bool>`；旧调用方不带 key 时行为不变。
  2. `POST /api/source-server-cancel-profile {requestId}` 只向该 caller 的 watch 置位，返回
     `{cancelled: boolean}`（未知/已完成 key 返回 `false`，幂等、无副作用）。
  3. `connect_cancellable` 用 `select!` 竞争取消信号与 `connect` future。取消时丢弃 future：
     transport `Child` 的 `kill_on_drop(true)` 关闭 SSH/WSL/Docker socket；随后递增 attempt 使迟到结果失效、
     移除未完成的 Provider Provisioning、发布 `offline` + `errorCode=remote-connection-canceled`。
  4. resolver 结束时（无论成功/失败/取消）注销 caller key，避免取消表泄漏。
- 错误码：`remote-connection-canceled`（已在 `PUBLIC_ESCODE_ERROR_CODES`），UI 归类为 canceled，不展示为
  可重试故障。取消后重新提交同一 target 必须建立新连接，不复用被取消的 pending。
- UI 取消语义（2026-10-31 实测确认）：连接中按 Escape/关闭并确认「停止连接」会直接关闭向导，
  不进入错误页，也不产生 logical session 或 tab；改凭据重试须重新进入向导，不能复用
  `pendingRemoteTarget` 里的旧凭据。
- E2E 代理观测条件（2026-10-31 实测确认）：SSH 客户端建连即写入 banner，取消时子进程被 SIGKILL，
  FIN 排在未读 banner 之后。若代理不消费收件箱（未 `resume()`），暂停的 readable 侧读不到 EOF，
  Node 只停在 `CLOSE_WAIT` 且永不触发 `end`/`close`，会被误判成「socket 未关闭」。
  `createFirstConnectionStallingSSHProxy` 因此对首连接 `resume()` 丢弃字节，关闭仍只以真实 FIN 为准。
- 回归：`source_server_connections_cancel_tests.rs`（取消中断在途连接、未知 key 返回 false、取消后重连成功）、
  `remote_scope_tests.rs`（`requestId` 解析）、`desktopRemoteSessionsRustHostMode.test.ts`（单 caller 取消
  中止 fetch 并调用 Host cancel；共享 flight 时保留其它 caller），以及真实 SSH E2E
  `conversation-session-ssh-remote-cancel-retry.test.ts`（取消后首个 SSH socket 关闭、向导关闭且无新 tab，
  重新进入向导用正确凭据连接成功；Linux `desktop-e2e-20261031-085743-605` 1 passing）。

**S5-e 远端项目归属与 `project` channel（2026-10-31）**

- 触发：`SSH-P0-PROJECT-01` 在 S5-a identity 修复后仍不渲染 `chat-remote-folder`；live fiber 显示
  `ProjectDirectory` 的 `service` / `remoteService` 均为 `undefined`。
- 根因（两个独立缺口）：
  1. Renderer 的 `RemoteServiceAccess` 没有绑定 `project` / `task-groups` / `remote-projects`。
     Rust Host 已注册这三个 channel，但 `IServiceAccessor` 字段可选，漏绑只会静默得到 `undefined`。
  2. `project` channel 曾把非 `local` 的 `hostId` 转发到 Source 的同名 channel。本 fork 的 Source
     Server 是 Node stdio server（`packages/server/src/entry-stdio.ts`），其 `createLocalServices`
     不注册 `project`；因此远端没有项目域，转发必然 `Unknown channel`。
- 所有权（本 fork 的实现选择，与上游“远端 Host 持有远端项目库”不同）：
  - `projects` / `project_roots` / `tasks` / `remote_project_entries` 的唯一写者仍是控制端 Rust
    `tasks-index.sqlite`；`projects.host_id` 区分 `local` 与 `remote:server:<sourceId>`。
  - 远端只提供文件系统事实：`readDirectory`、`saveProject` 的 root 校验、`prepareSession` 的
    主目录复检都经在线 Source 的 `file.stat` / `file.readdir`。
  - 离线时显式 `project.host-offline`；不回退本地文件系统，不创建替代 Host。
  - `prepareSession({hostId: remote, projectId})` 要求项目存在且恰好一个 root，返回
    `workspaceIdentity = <hostId>:<posix-normalized-path>`；本地 scope 仍不填 identity。
  - 客户端 `hostId` 不能改变已有项目的 `host_id`；远端 `deleteProject` / `reorderProjects` /
    `moveTask` 要求 Source 在线。
- 草稿归属（2026-10-31 真实 SSH E2E 补充）：`ProjectEditor` 保存成功、Host `prepareSession` 返回
  `projectId` 后，Root 的 `onWorkspaceActivated` 必须把该 `projectId` 交回 session store 的
  `startDraft(..., { projectId })`。此前 `handleRemoteWorkspaceActivated` 丢弃了回调里的
  `projectId`，只创建 tab 和未绑定项目的草稿，`draftProjectId` 保持 `null`，项目侧栏与首发无法
  关联。Owner 不变：Host / `tasks-index.sqlite` 持有项目事实，Renderer session store 持有草稿归属。
- 未做（保持与本轮之前的本地行为一致）：项目写成功后仍不发布 `state:projects` 广播；UI 依赖显式
  `sidebar.refresh()`。跨窗口自动刷新列为后续项。
- 回归：`SSH-P0-PROJECT-01` 断言 `remote:server:` identity、非空 `draftProjectId`、草稿
  `activeTaskId` 为 `null` 且侧栏出现项目行；`remoteServiceAccess.test.ts`（三个 channel 绑定）、
  `tasks_index::project_tests::remote_projects_are_partitioned_by_host_and_single_root`
  （按 host 分区、远端单 root、host 不可变）、`domains_project_remote::tests`
  （identity 归一化）、`domains_project`（远端离线显式失败、本地不误路由）。

### 12.3 切换顺序（每步可回滚）

```text
1. 直接首连 target → Rust Host（Main）                         [已完成，Linux SSH 真实 E2E：desktop-e2e-20261031-122307-136]
2. 移除 node-host route 的创建；保留读取旧 route 的兼容代码一个版本  [已完成（2026-11-01）：非 server Host backend 分支删除；server 目标 Main 侧 Node 路由删除（S5-f-4），Host 侧 handler/注册表删除（S5-f-5）]
3. 删除 ProviderProvisioningExecute / ExecutionResult 远端分支     [已完成（2026-11-01）：Main 发送端（S5-f-4）与 Host 处理端、共享 schema（S5-f-5）均已删除]
4. 删除 renderer 远端 MessagePort 通道选择                        [已完成（2026-11-01）：`attachRemoteWorkspaceSessionHost` 删除（S5-f-4）；Host 侧 remote attachment 仅保留显式失败（S5-f-5）]
5. 删除 ESCODE_RUST_HOST=0 远程分支                              [已完成（S5-b）]
6. 删除 Node remote deploy / preflight / installer（先 dep:refs 证明）[已完成（2026-11-01）]
7. 清理文档、脚本、fixture 中的 Node 远程路径引用                  [已完成（2026-10-31；保留历史记录与 Web kind:"server" 说明）]
```

**S5-c / S5-d 调查记录（2026-10-31）**

- 路由事实：`resolveRemoteTarget` 只对 `kind: "server"` 返回无 `profileScope` 的 Node 路径；Node Host
  `handleConnected` 对非 server descriptor 直接拒绝并 dispose，`attachRemoteWorkspaceSessionHost` 也只接受
  `owner: "node-host"` + `target.kind === "server"`。因此 SSH/WSL/Docker 既不创建 Node 路由，也不会进入
  `ProviderProvisioningExecute` 或 renderer MessagePort 远端 attachment。
- 结论（S5-c）：`packages/desktop/src/host/index.ts` 的 `ProviderProvisioningExecute` 与 renderer 远端
  MessagePort 选择仍被 `kind: "server"` 使用，删除会破坏保留的 Web 服务端路径；SSH/WSL/Docker 的
  Provider Provisioning 已由 Rust Host 的 `SourceServerProviderProvisioning` 承担，两条路径不重叠。
- 结论（S5-d）：`packages/server/src/http.ts` 的 `/api/connect-remote` 仍执行
  `createRemoteBackend` → `connectRemote()`；后者除 `skipDeploy` 外必然调用 `deployServer`（`connect.ts`），
  因此 Node remote deploy / preflight / installer 仍是 Web 自连远端的生产依赖，不能按目录名删除。
- 构建期还有独立依赖：`packages/desktop/scripts/prepare-bundled-remote-assets.ts` 引用 `localTarGz`、
  `remoteAssetCdn`、`escodeAgentOfficialPluginAssets`、`serverBundleDeployCheck`、`escodeAgentBundleWrapper`。
  即使将来退役 Web 自连，这些模块也需保留或迁移，不能随 `src/remote` 目录整体删除。
- 待办：S5-c/S5-d 的删除以「退役或迁移 Web `/api/connect-remote`」为前置；在那之前保留代码并继续用
  `pnpm dep:refs` 复核，不以 `pnpm knip` 的仓库级噪声作为删除依据。

**S5-c / S5-d 更新记录（2026-11-01）**

- 前置解除：`packages/web`（唯一调用 `POST /api/connect-remote` 的 Web 客户端）已在 2026-10-20
  随手机远控下线删除（`1df037e6`）；`rg "api/connect-remote"` 与 `rg "connectRemote"` 在
  生产代码中只剩服务端 route 自身和 Desktop IPC 的 `kind: "server"` 直连，没有 Web 调用方。
- 已删除：`packages/server/src/http.ts` 的 `POST /api/connect-remote` 与 `GET /ws/remote/:id`
  配对入口，以及仅服务于它们的 `remoteConnections` / `generateId`；`serverInfoHttp.test.ts`
  断言两条 route 均返回 404，避免重新引入 Server 临时 stdio 主链。
- 删除边界：`packages/desktop/scripts/prepare-bundled-remote-assets.ts` 仍消费
  `localTarGz`、`remoteAssetCdn`、`escodeAgentOfficialPluginAssets`、`serverBundleDeployCheck`、
  `escodeAgentBundleWrapper`，为 Rust Host 首装准备 release 资产；`kind: "server"` Desktop 直连
  仍使用 Host 内的 `connectServerRemote`，不经过 `packages/server/src/remote` 的 deploy 链。
- 已删除（S5-d）：`connect.ts`、`handshake.ts`、`stdio-socket.ts`、`deploy.ts`、`deployShared.ts`、
  `escodeAgentDeploy.ts`、`escodeAgentDevDeploy.ts`、`escodeAgentWrapperDeploy.ts`、
  `escodeAgentOfficialPluginPermissionRepair.ts`、`runtimeToolDeploy.ts`、`remoteAssetDeployDecision.ts`、
  `remoteAssetInstaller.ts`、`remoteAssetPreflight.ts`、`remoteAssetCache.ts`、`remoteAssetLiveIdentity.ts`、
  `remoteAssetNetwork.ts`、`remoteDeployLock.ts`、`remotePlatformSupport.ts` 及对应旧测试；
  `packages/desktop/src/host/index.ts` 的 `setupRemoteConnection` 与 backend 服务分支同步删除，
  非 server 目标在 Node Host 显式返回 `REMOTE_RUST_HOST_REQUIRED`。
- 保留：`backend.ts` / `ssh-backend.ts` / `wsl-backend.ts` / `docker-backend.ts` / `create-backend.ts`
  的 exec/upload/detect 原语，以及构建期资产工具（`localTarGz`、`remoteAssetCdn`、`serverBundleDeployCheck`、
  `escodeAgentBundleWrapper`、`escodeAgentOfficialPluginAssets`）；Rust Host 首装读取同一 release 目录。
- 删除后门禁（2026-11-01 本机 Linux x64）：`pnpm typecheck`、`pnpm lint`（202 条既有 warning / 0 error）、
  `pnpm architecture:check --changed`、`node scripts/check-escode-host-boundaries.mjs`、
  `pnpm check:escode-host`、`pnpm test:escode-host`（workspace 全量 + interop 2 例）与
  `pnpm build:bootstrap` 全部通过；`provider-provisioning-execute` 与远端 MessagePort 仅在
  `target.kind === "server"` 分支可达。

- 每步保持「同一 workspace 只有一条在线路径」，不允许双 Host 同时服务。
- 删除前运行：`pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed`、
  `pnpm check:escode-host`、`pnpm test:escode-host`，以及首连、断线、重启、多窗口、更新、Provider、迁移
  E2E。
- 仍引用旧路径的 pending case（如 `manual-review/pending/*`）必须同步迁移或标注删除；受保护文件按任务
  约定单独处理。

### 12.4 验收场景（S5）

1. 删除后首连、断线原位重连、App 重启恢复、多窗口共享实例全部走 Rust Host，无 Node UtilityProcess
   远程进程。证据（Linux，S5-f-5 删除 Node 远程路径之后复跑）：首连
   `desktop-e2e-20261031-235507-219`；多窗口 + 断线重连 + 重启恢复
   `desktop-e2e-20261031-235726-737`（3 例）；远端 scope + Provider Provisioning
   `desktop-e2e-20261101-001114-778`；`kind: "server"` 真实 server
   `desktop-e2e-20261031-225816-501`；均断言零 `escode-host-remote-*`。删除前的历史证据见
   `desktop-e2e-20261031-122307-136` / `-123400-961` / `-125229-399`。Windows/macOS 待对应平台
   环境（见 §11.11）。
2. `ESCODE_RUST_HOST=0` 下远程连接仍成功（开关只影响本地），或按产品决策显式报错而非回退 Node。
   证据（2026-10-31）：S5-b 已让远程路由不再读取该开关；
   `desktopRemoteSessionsRustHostMode.test.ts` 新增用例断言缺少 Rust resolver 的 SSH 目标返回
   `REMOTE_RUST_HOST_UNAVAILABLE` 且不调用 `ensureWindowHost`，即显式失败而非回退 Node。
3. 直接首连（无 `profileId`）同样落在 Rust Host，并能回填 `sourceServerId` / `profileId` 供重启恢复。
   证据：`SOURCE_SERVER_RUST_FIRST_CONNECT`（Linux SSH，`desktop-e2e-20261031-122307-136`）断言 workspace
   identity 收敛到 Source Server、Profile 持久化 `sourceServerId` 与
   `passwordCredentialKey = connection-profile:<profileId>:password`、`preferredProfileId` 已绑定且状态
   `online`，同时不存在 `escode-host-remote-*` 远程进程。
4. 同一 target 并发首连只产生一个 profile / 一条连接。证据：
   `source_server_profile_ensure_tests::concurrent_first_connect_creates_one_profile`（Rust registry 单飞）
   与 `desktopRemoteSessions` 的 `startProfileResolutionFlight`（Main 按 `profile:<id>` / target 身份键
   request-level single-flight）。
5. `rg` 与 `dep:refs` 证明 SSH/WSL/Docker 启动入口已无 Node 远程引用；Web 自连入口已退役
   （2026-11-01 更新）：
   - Web 自连：`POST /api/connect-remote` 与 `/ws/remote/:id` 已删除；`serverInfoHttp.test.ts`
     断言两者返回 404，`rg "api/connect-remote"` 不再命中生产调用方。
   - Host 侧：`ProviderProvisioningExecute` 的 Main 发送端与 Host 处理端已删除（S5-f-4/S5-f-5），
     SSH/WSL/Docker 全部由 Rust Host 的 `SourceServerProviderProvisioning` 承担。
   - 测试：`packages/desktop/test/desktopRemoteSessionsRustHostMode.test.ts`（server 走 resolver、
     缺 resolver 显式失败），`packages/shared/test/windowHostControlProtocol.test.ts`（保留的 host 协议面）。

### 12.5 完成判据

> 当前阻塞（2026-11-01）：其一，Windows/macOS 主链 E2E 需要对应平台桌面环境或可达 SSH 目标（本机只有 Linux x64，CI 的 SSH job 固定 `ubuntu-latest`，见 §11.11）。其二，零 `escode-host-remote-*` 的 server 真实 E2E、两窗口共享实例与重启恢复已在 Linux x64 通过（`source-server-rust-server-direct.test.ts`，artifact `desktop-e2e-20261031-225816-501`），跨平台版本仍待补齐。S5-d 的 Node remote deploy 链已删除，但 S5 与总目标尚未完成，不得据此删除 Node Host / TS CLI。
>
> 2026-11-01 补充：run `41282629407`（commit `f6f4b10f`）已让 `pnpm test:escode-host` 在
> windows-latest / macos-latest 转绿，`desktop e2e matrix` 的
> `source-server-rust-server-direct.test.ts` 三平台全绿——上面第二条（server-direct 跨平台版本）
> 因此补齐。第一条（SSH 三条主链）仍缺对应平台可达目标机，P1 的真实供应商/MCP/安装包演练也未开始，
> S5 与总目标仍未完成。

- S2–S4 验收全绿（含 Windows/macOS 主链）。
- 删除后的 E2E 矩阵全绿，SSH/WSL/Docker 日志中不再出现 per-window Node Host、`ProviderProvisioningExecute`
  或 `ESCODE_RUST_HOST=0` 远程分支。Desktop `kind: "server"` 直连已按 §12.6 迁到 Rust Host；Node Host
  的 `connectServerRemote` 与 Provider Provisioning 处理端已于 S5-f-5 删除，不得反向成为任何 transport
  的 Node 回退。
- 文档、技能、脚本无失效引用；§7 的 S5 项标记完成并记录证据 run id。

### 12.6 S5-f：`kind: "server"` 直连迁到 Rust Host（设计，2026-11-01）

> 设计记录：2026-11-01，处理 §12.5 的第二个阻塞项。选择「补齐 Rust transport」而不是「退役入口」：
> `kind: "server"` 是连接向导的 dev-only 传输（`buildAvailableKinds` 只在
> `isLocalDevelopmentRuntime` 时加入），退役等于删除仍在使用的自托管/开发能力，需要产品确认；
> 补齐后它与 ssh/wsl/docker 共用同一套 registry、实例表、状态机与恢复策略，per-window Node Host
> 才可能整体删除，也才符合「同一 workspace 只有一条在线路径」。

#### 12.6.1 分层：差异只在「链路」

| 环节     | ssh / wsl / docker                       | `kind: "server"`                                |
| -------- | ---------------------------------------- | ----------------------------------------------- |
| 启动     | `TransportCommand` 拉起远端 stdio        | 不拉起进程；HTTP 探测 + WebSocket 升级          |
| 握手     | `escode-hello` / `escode-hello-ack` 行   | 无 hello；`GET /api/server-info` 提供版本与身份 |
| 帧面     | `rpc_wire` Regular 帧                    | 同一帧面，WS 二进制消息就是帧字节               |
| 链路之上 | `RemoteChannelClient` → channel → 实例表 | 完全一致，不新增第二条业务路径                  |

```
Main/Renderer              Rust Host                                escode-server
    | target.kind=server      |                                          |
    |------------------------>| 1. GET  {base}/api/server-info           |
    |                         |    (authorization: Bearer + token query) |
    |                         |----------------------------------------->|
    |                         | 2. POST {base}/api/rpc-host-capability   |
    |                         |----------------------------------------->|
    |                         | 3. WS   {base}/ws/host                   |
    |                         |    + x-zcode-rpc-host-capability         |
    |                         |<========================================>|
    |                         |    rpc_wire 帧面（与 stdio 分支同一实现） |
```

#### 12.6.2 Profile 与凭据

- `transport = "server"`，`target = { url, tokenCredentialKey? }`。
- token 只写 `CredentialService`（`connection-profile:<profileId>:token`），settings 只保存引用；
  URL 与 token 不写日志。
- `ensure_profile` 用 trim 后的 `target.url` 原样作为 target 身份键（不做协议互转，避免把
  `https://x` 与 `wss://x` 判成同一目标）；同一 target 的并发首连继续 single-flight。

#### 12.6.3 端点解析

`resolve_server_remote_endpoints(url)` 与 TS `resolveServerRemoteEndpoints`
（`packages/desktop/src/host/serverRemoteConnection.ts`）逐字节一致：

1. `trim` 后为空 → 显式错误（server URL 必填）。
2. `http`/`https` 派生 `ws`/`wss`；`ws`/`wss` 派生 `http`/`https`；其它协议显式错误。
3. 四个 URL 各自先剥离尾部 `/ws/host` 或 `/ws`，再去掉尾部 `/`，然后拼 `/api/server-info`、`/ws`、
   `/api/rpc-host-capability`、`/ws/host`。
4. 清空 `search` / `hash`。

#### 12.6.4 鉴权与协议版本

- 有 token 时 `/api/server-info` 与 `/api/rpc-host-capability` 都带 `authorization: Bearer <token>`，
  同时把 token 放进 query（与 TS 一致，兼容只读 query 的 server）。
- `/ws/host` 升级请求同时带 `authorization: Bearer`、`x-zcode-rpc-host-capability: <capability>`
  与 token query；capability 有有效期，重连重新申请，不复用旧值。
- `protocolVersion !== 1` → `source-server-incompatible`，不降级、不猜协议。
- HTTP 非 2xx、响应体不合 schema、WS 升级失败都返回显式错误（带状态码或原因），不重试成回退。
- 代理策略沿用 `escode-cli-net`（系统代理 + `no_proxy`），loopback 直连，不额外读取环境变量。

#### 12.6.5 安装、身份与 host key

- server 目标不执行 `prepare_install` / `apply_install` / 更新：runtime 由服务端自己持有。
  对 server 目标发起显式更新返回 `source-server-update-unsupported`，不静默跳过。
- 身份：`sourceServerId = info.serverId`，`version = info.version`，`workspaces` 由
  `info.workspaces` 投影为 `DeclaredWorkspaceInput`（保留 `workspaceIdentity` / `label`）；
  `os_user` / `hostname` 为 `None`（服务端不披露）。
- 无 SSH → 不读 `known_hosts`、不产生 `remote-host-key-changed`；
  `source-server-identity-change-pending` 规则不变（`serverId` 变化同样需要确认后重绑）。

#### 12.6.6 失败语义与所有权

- 任一环节失败都在 Rust Host 状态机落 `error` + 错误码，Main 不回退 per-window Node Host。
- server 目标与 ssh/wsl/docker 共用 `SourceServerConnections` 实例表：同一 `sourceServerId` 的同一
  workspace 只允许一个实例，多窗口共享订阅。
- `desktop-continuous` 语义不变：断线后原位重连由 Rust Host 负责，Renderer 只保留未提交草稿与
  pending optimistic overlay。

#### 12.6.7 验收场景

| #   | 场景                                                            | 证据                                                                                            |
| --- | --------------------------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| 1   | 端点解析：http/https/ws/wss、子路径、尾部 `/ws`、空白、非法协议 | Rust 单测，逐条对齐 TS `serverRemoteConnection.test.ts`                                         |
| 2   | info/capability 鉴权头与 capability 头升级成功                  | Rust 集成：本地 `escode-server` fixture                                                         |
| 3   | Initialize 后 channel 可用，WS 断开时 `closed` 信号置位         | 同上                                                                                            |
| 4   | 首连回填 `profileId` / `sourceServerId`，App 重启恢复并原位重连 | Desktop E2E `source-server-rust-server-direct.test.ts`（真实 `escode-server`），2026-11-01 通过 |
| 5   | 同一 workspace 双窗口共享同一实例                               | 同上（第二窗口恢复同一远端 tab）                                                                |
| 6   | 失败显式报错，且不出现 Node UtilityProcess 远程进程             | 同上：断言零 `escode-host-remote-*`                                                             |
| 7   | 删除 node-host 分支后 `rg` / `dep:refs` 无生产引用              | 命令输出登记                                                                                    |

#### 12.6.8 切片

| 切片   | 内容                                                                              | 退出条件                                      |
| ------ | --------------------------------------------------------------------------------- | --------------------------------------------- |
| S5-f-1 | 端点解析 + `TransportTarget::Server` 解析（纯函数）                               | §12.6.7-1                                     |
| S5-f-2 | info/capability + WS 链路接入 `RemoteConnection`（无子进程）                      | §12.6.7-2/3                                   |
| S5-f-3 | `establish` 按 transport 收口 install/probe，身份从 info 投影                     | 单测 + 兼容判定                               |
| S5-f-4 | Main 把 server target 路由到 `source-server-resolve-profile`，删除 node-host 分支 | TS 断言不调用 `ensureWindowHost`              |
| S5-f-5 | E2E + 文档收口                                                                    | §12.6.7-4/5/6/7（Linux x64 已过，跨平台待补） |

**S5-f-2 实现记录（2026-11-01）**

- `source_server_endpoints`（S5-f-1）：`resolve_server_remote_endpoints` / `authenticated_url`，
  12 例单测覆盖协议互转、子路径、尾部 `/ws`、非法协议与 token 覆盖语义。
- `source_server_info`：`parse_server_info` / `parse_host_capability`。`protocolVersion != 1`、
  `capabilities.desktopContinuous|websocketRpc` 非 `true`、必填字段缺失都显式失败。
- `source_server_websocket`：`probe_server`（GET/POST + Bearer/query token）、`open_websocket`
  （`x-zcode-rpc-host-capability` + Bearer + token query，`wss` 用 ring provider）、`bridge`
  （WS 二进制消息 ↔ `rpc_wire` 帧 ↔ `RemoteChannelClient`，关闭时置位 `closed`）。
  capture 只记录去掉 token 的 URL。
- `RemoteConnection` 改为链路无关：`info: ConnectionInfo`（stdio hello 或 server-info 投影）
  - `child: Option<Child>`；server 目标的 `platform` / `arch` / `pid` 保持 `None`，不编造占位值。
- 验证：`pnpm check:escode-host`、`pnpm test:escode-host`（69 个测试目标全部 ok）、
  `source_server_websocket` 8 例、`source_server_info` 8 例、`source_server_endpoints` 12 例。
  尚未接入 `establish`（S5-f-3）与 Main 路由（S5-f-4）。

**S5-f-3 实现记录（2026-11-01）**

- `ResolvedTransport` 改为 `link: RemoteLink`（`Process(TransportCommand)` / `Server(ServerConnect)`）
  与 `runner`；server 链路用 `unavailable_runner()`，安装/更新对 server 目标显式拒绝
  （`source-server-update-unsupported`），不静默跳过。
- 生产 resolver（`domains_source_server_options`）先解析 target：server 目标只解 token 引用即返回
  `ResolvedTransport::server`，不再要求 askpass 路径；ssh/wsl/docker 行为不变。
- `establish` 保持单条写入路径：安装只对 `Process` 生效；身份顺序不变（`Process` 读 install.json，
  `Server` 读 `/api/server-info`），身份变化仍先返回 `identity-change-pending` 再开链路。
  `workspaces` 由 `AnnouncedIdentity` 摊平（stdio 单路径 / server-info 多条，保留 label 与 identity，
  绑定前统一 canonical 化）。
- `open_connection` / `connect_initialized` 接受 `RemoteLink`：`Server` 用调用方已取得的探测结果升级
  并复用同一 Initialize 超时；`open_raw_connection`（更新探针）仍只服务子进程链路。
- 验证：`pnpm check:escode-host`、`pnpm test:escode-host`（69 个测试目标全绿）；
  新增 `server_link_establishes_and_binds_identity`（HTTP+WS 同端口 fixture，断言 online、
  `sourceServerId`、`currentVersion`、两条 workspace 绑定与 canonical identity）与
  `server_link_reuses_the_existing_connection`。

**S5-f-4 设计记录（2026-11-01）**

目标：Main 不再区分 `kind: "server"` 与 ssh/wsl/docker 的路由；所有远程 target 都先经
`POST /api/source-server-resolve-profile` 得到 `profileId` / `sourceServerId`，再申请 Rust Host 的
scoped capability。per-window Node Host 的远程 workspace 分支随之删除。

- Connection Profile 增加 `transport = "server"`（`target = { kind: "server", url, tokenCredentialKey? }`）：
  - `shared/src/source-server/contracts.ts` 的 `sourceServerTransportKindSchema`、
    `sourceServerProfileTargetSchema`、`connectionProfileSchema` 同步扩展，否则 TS 读回 Rust 写入的
    settings 会整份失败。
  - `RemoteTarget` 的 server 形态补 `profileId` / `sourceServerId`，与其它 transport 共用
    `ProfileScopedRemoteTarget`；`serverConnectOptionsSchema` 同步允许这两个字段。
  - Rust `normalize_target` 只保留 `url`（trim）；Profile 身份键与 spec §12.6.2 一致，就是 trim 后的
    `target.url`，不把 `https://x` 与 `wss://x` 归一成同一目标。
  - token 只写 `CredentialService`（`connection-profile:<profileId>:token`），settings 只保存
    `tokenCredentialKey`；复用 Profile 且本次未带 token 时保留旧引用，不清空。
- Main 路由：`resolveRemoteTarget` 删除 `if (target.kind === "server") return { target }` 分支，
  统一走 resolver/single-flight；`remoteScopeOf` 对 server 也读 `target.profileId`；
  `createRemoteWorkspaceSession` 只保留 Rust scope 路径，缺 resolver 时显式
  `REMOTE_RUST_HOST_UNAVAILABLE`。
- 删除范围（§12.1 第 1/3/4 项）：
  - `desktopRemoteSessions`：`owner: "node-host"` route、`windowHostProcessMap` / `ensureWindowHost`、
    `ConnectRemoteWorkspace` / `DisposeRemoteWorkspaceSession` 远端分支、`ProviderProvisioningExecute`
    发送端、`attachRemoteWorkspaceSessionHost`（仅导出、无生产调用方）。
  - `packages/desktop/src/host/index.ts`：per-window Node Host 的远程连接注册表、
    `windowRemoteConnectionRegistry`、`ProviderProvisioningExecute` 处理与 remote scope attachment 分支。
  - `packages/shared`：`ConnectRemoteWorkspace` / `CancelRemoteWorkspaceConnect` /
    `BindRemoteWorkspaceContext` / `DisposeRemoteWorkspaceSession` / `ProviderProvisioningExecute` 及对应
    响应 schema/枚举；`AttachServicePort` 仅保留 local scope。
  - `packages/desktop/src/host/serverRemoteConnection.ts`：Rust 链路（§12.6.1）已覆盖 GET info、POST
    capability、WS 升级与帧泵，删除前用 `pnpm dep:refs` 复核无生产引用。
- 保留：`packages/desktop/src/renderer/src/remoteWorkspaceSessionServices.ts` 的
  `buildServerRemoteWorkspaceSessionServices`——它描述 server 目标“只使用远端服务面”的语义，与链路
  实现无关，Rust route 仍然适用。
- 验证（2026-11-01 本机 Linux x64）：`pnpm typecheck`、`pnpm lint`（201 条既有 warning / 0 error）、
  `pnpm architecture:check --changed`、`pnpm check:escode-host`、`node scripts/check-escode-host-boundaries.mjs`
  全部通过；Rust `source_server::writes::profile_ensure` 8 例通过（新增 server token/身份键/保留引用 3 例）；
  `desktopRemoteSessionsRustHostMode.test.ts` 4 例（server 走 resolver、缺 resolver 显式失败、取消语义）、
  `remoteWorkspaceSessionServices`、`windowHostAttachmentRegistry`、`windowHostControllerService`、
  `windowRemoteConnectionRegistry`、`serverRemoteConnection`、`windowHostControlProtocol`、
  `remoteProviderProvisioningService`、`remoteWorkspaceServicePortBridge` 共 130+ 例通过。
  已知既有失败：`packages/ui/test/sshDialogFlow.test.ts` 3 例（profile 目标断言未含 S3-c 的
  `profileId`/`sourceServerId`），已用 `git stash` 在未改动的基线上复现，与本次改动无关。
- 未完成：`packages/desktop/src/host/index.ts` 的 remote registry / `ProviderProvisioningExecute` /
  `serverRemoteConnection.ts` 删除，以及 `ConnectRemoteWorkspace` 等共享协议项的移除，随 S5-f-5 的
  `rg`/`dep:refs` 证据一起收口。

**S5-f-5 实现记录（2026-11-01，Node Host 远程路径删除）**

目标：`kind: "server"` 与 ssh/wsl/docker 一样只走 Rust Host 后，per-window Node Host 的远程分支已无
生产调用方；本切片删除该分支及其协议面，Node Host 只保留本地服务。

- `packages/desktop/src/host/index.ts`（2272 → 约 1750 行）：
  - 删除 `windowRemoteConnectionRegistry`、`connectServerRemote`、`remoteConnectionProgressContext`、
    `createHostRemoteWorkspaceProxyState` 的远程接线，以及 `HostRemoteConnectionCapabilities`。
  - 删除 handler：`ConnectRemoteWorkspace`、`CancelRemoteWorkspaceConnect`、`BindRemoteWorkspaceContext`、
    `DisposeRemoteWorkspaceSession`、`ProviderProvisioningExecute`。
  - `AttachServicePort` 不再 await 远程 scoped services；收到 `kind: "remote"` scope 时显式抛
    `Remote workspace attachment is served by the Rust Host`，不静默回退本地服务。
  - `materializeRecording` / `resolveAutomationTargetServices` 对 `remoteSessionId` 显式失败。
  - shutdown 阶段移除 `remote-registry-dispose`；拓扑日志只保留 attachment 数。
- 删除文件（`rg` 复核无生产引用）：`host/serverRemoteConnection.ts`、`host/windowRemoteConnectionRegistry.ts`、
  `host/remoteDynamicWorkflowUserMode.ts`、`host/remoteWorkspaceServiceCollection.ts`、
  `host/remoteProviderProvisioningService.ts`、`host/remoteMediaPreviewProxy.ts` 及其 helpers、
  `server/src/remote/remoteConnectionProgressContext.ts`；对应 7 个测试文件同步删除
  （`hostRemoteWorkspaceServices.test.ts` 覆盖的 server service collection 一并退役）。
- `packages/shared`：删除 `ConnectRemoteWorkspace` / `CancelRemoteWorkspaceConnect` /
  `BindRemoteWorkspaceContext` / `DisposeRemoteWorkspaceSession` / `ProviderProvisioningExecute` /
  `RemoteWorkspaceConnected` / `ConnectFailed` / `ContextBound` / `ContextBindFailed` / `Closed` /
  `ConnectionLog` / `Acquired` / `ProviderProvisioningSourceChanged` / `ProviderProvisioningExecutionResult`
  枚举与 zod schema、`remoteAssetDirsSchema`。`windowHostControlProtocol.test.ts` 重写为保留面
  （CUA PiP focus、attachment scope、AttachServicePort）。
- 保留与迁移：
  - `resolvedWslTargetSchema` 从 `ConnectFailed` 响应中抽出为独立导出——renderer 的安装/升级等待仍需
    校验“本次已解析 WSL 身份”，`requestRemoteProjectConnection` 改用它。
  - `windowHostAttachmentScopeSchema` 保留 `kind: "remote"` 形态，仅用于 Node Host 显式拒绝；生产侧
    已无 remote scope 发送方（Main 走 Rust scoped capability）。
  - `hostRemoteWorkspaceProxyState.ts` 保留：本地 `ESCodeTaskService` 的 reporting/session-route 包装仍使用。
  - `renderer/src/remoteWorkspaceSessionServices.ts` 的 `buildServerRemoteWorkspaceSessionServices` 保留
    （只描述 server 目标使用远端服务面的语义）。
- 证据（2026-11-01，本机 Linux x64）：
  - `rg -n "ConnectRemoteWorkspace|…|ProviderProvisioningExecutionResult" packages apps` 仅剩
    `resolveRemoteWorkspaceClosedFallback`（UI 历史恢复辅助，与协议无关）与
    `onProviderProvisioningSourceChanged`（本地 Source 持久化回调，仍在用）。
  - `rg -l "windowRemoteConnectionRegistry|remoteDynamicWorkflowUserMode|remoteWorkspaceServiceCollection|remoteMediaPreviewProxy|remoteConnectionProgressContext"` 命中 0；
    `serverRemoteConnection` 命中仅剩 Rust 端点文件，`remoteProviderProvisioningService` 仅剩 UI 否定断言。
  - `pnpm typecheck` 通过；`pnpm lint` 196 条既有 warning / 0 error；`pnpm architecture:check --changed` OK；
    `pnpm exec vitest run` 覆盖 `desktopRemoteSessionsRustHostMode`、`windowHostAttachmentRegistry`、
    `windowHostControllerService`、`remoteWorkspaceSessionServices`、`remoteConnectionFlowRecovery`、
    `settingsWorkspaceServiceRouting`、`validation`、`windowHostControlProtocol` 共 182 例通过。
- 真实 `escode-server` 的 server 直连 E2E（首连、应用重启恢复、断线原位重连、两窗口共享实例、
  零 `escode-host-remote-*`）已在 Linux x64 通过：`source-server-rust-server-direct.test.ts`，
  artifact `desktop-e2e-20261031-225816-501`。期间修复了 WS Initialize 早于 channel 注册的能力集
  缺陷（见上一节问题记录）；E2E 的 Host 进程基线改为在 App 重启之后取（重启会换 Rust Host 进程）。
- 删除后 SSH/server 远端链路复跑（2026-11-01，Linux x64，本机 Docker sshd + 真实 escode-server）：
  - `source-server-rust-first-connect`（`desktop-e2e-20261031-235507-219`）
  - `source-server-rust-multi-window` + `-transport-drop` + `-restart-recovery`
    （`desktop-e2e-20261031-235726-737`，3 例）
  - `source-server-rust-install`（`desktop-e2e-20261101-000342-581`）与
    `source-server-rust-host-key`（`desktop-e2e-20261101-000052-414`）
  - `source-server-rust-remote-scope` + `source-server-rust-provisioning`
    （`desktop-e2e-20261101-001114-778`）
  - `source-server-rust-server-direct`（`desktop-e2e-20261031-225816-501`）
    全部断言零 `escode-host-remote-*`；`ESCODE_RUST_HOST` 仍只作用于本地 Host 选择（§12.2），
    远程链路不读取该开关。
- 单测门禁（2026-11-01）：`pnpm verify:pre-push` 全绿（1573 files / 13874 tests passed，27 skipped）。
  同一轮补齐了 `daaa2043` 引入的 tasks 账本引导缺口（Rust Host `rpcListen` 要求 Main 先建
  `tasks-index.sqlite`）与 S3-c/S5-f-5 协议删除后失效的断言，共 28 例，见 commit `fd4e30a6`。
- 未完成：Windows/macOS 主链 E2E（§11.11 的 CI SSH job 固定 `ubuntu-latest`）；在跨平台证据补齐前
  S5 与总目标仍不得标记完成。

**S5-f-5 问题记录：WS 首帧 Initialize 早于 channel 注册（2026-11-01）**

真实 `escode-server` 直连 E2E（§12.6.7-4）首次执行时，Rust Host 在首连 provisioning 屏障处失败：

```text
[rust-host] attachment capability failed Rust Host refused to issue a remote workspace capability (HTTP 400):
provider provisioning unsupported: capability-unavailable
```

- 触发场景：Desktop 用持久 Profile（`transport: "server"` + 凭据）连接真实 `escode-server`，
  `establish` 完成 Initialize 后进入 `provision_before_online`，`RemoteProvisioningPeer::supports_target()`
  读 `RemoteChannelClient::channel_names()` 判定远端没有 `provider-provisioning-target`。
- 根因证据：用 `@escode/rpc` 的 `ChannelClient` 直连 fixture（`GET /api/rpc-host-capability` →
  `WS /ws/host`，token + `x-zcode-rpc-host-capability`）后，首个 Initialize 帧的 `channels` 不含
  `provider-provisioning-target`、`escode-agent` 等任何业务 channel；同一次连接里
  `[rpc:register] channel "provider-provisioning-target"` 出现在 Initialize **之后**。
  `packages/server/src/http.ts` 的 `setupChannelServer` 用 `new ChannelServer(protocol, "server")`
  构造，默认 `deferInit=false` 会在构造期立即广播能力集，而 `services.exposeOnChannelServer()`
  还没注册任何 channel——与 `packages/server/src/stdio.ts` 已修复的是同一个时序缺陷（§10.11 问题记录）。
  Rust 侧 `wait_initialized()` 只取首帧能力集，后续不会因为 registe 补发而更新，所以远端 scope 只剩
  `window-controller`，provider provisioning 被判成 `capability-unavailable`。
- 修复：`setupChannelServer` 改为 `deferInit=true`，注册完 services/overrides 后调用
  `exposeServicesAndReady()` 再广播 Initialize；`/ws`（`web-remote-replayable`）与 `/ws/host`
  （`desktop-continuous`）共用同一收口点，能力集与 overrides（Provider Provisioning 仅受信 Host）
  保持同一份注册结果。
- 回归证据：`packages/server/test/httpInitializeCapabilities.test.ts` 断言两种模式的首帧 Initialize
  都广播已注册 channel（`/ws` 仍拿到显式拒绝的 override）；用原始实现运行该用例为
  `expected false to be true`，修复后通过。真实 server 探针（同上脚本）对 `/ws/host` 与 `/ws` 都返回
  `provider-provisioning-target: true`。随后重跑 §12.6.7-4/5/6 的 Desktop E2E：artifact
  `desktop-e2e-20261031-225816-501`，`1 passing (18.9s)`（本机 Linux x64）。

**S5-f-6 实现记录（2026-11-01，Node Host 远程 facade 残留审计与清理）**

目标：S5-f-5 删除 per-window Node Host 远程路径后，复核 §12.1 第 3/5 项残留是否仍可达；不可达的
远程 facade 一并删除，避免「没有调用方」的代码继续被误认为生产路径。

- 可达性审计（`rg` / `git show` 证据）：
  - `createReportingRemoteESCodeTaskService` 的唯一生产调用点（`host/index.ts` 初始化）只传
    `{ reportRunningPromptCount: false }`，`taskRealtimePort` / `materializePromptAttachments` 没有调用方。
  - 于是 `shouldWrapSendPrompt` 恒为 false，`sendPrompt` 包装从未安装：`mirrorRemotePrompt`、
    `prepareRemotePromptParams`、`beginWorkspaceTask` / `finishWorkspaceTask` 与
    `untrackedPromptRpcCount` 都是不可达的远端镜像分支。
  - `git show daaa2043 -- packages/desktop/src/host/index.ts` 显示该提交删除了
    `taskRealtimePort: activeSessionRealtimePort ?? undefined` 这一唯一消费点；此后
    `createTaskRealtimeBridgeForHostInit` 创建的 bridge 只在退出时 dispose，不再被任何服务使用。
- 删除：
  - `packages/desktop/src/host/index.ts`：上述 prompt 镜像分支、`isRemoteMirrorableStreamEvent`、
    `activeSessionRealtimePort` 及其两处 dispose。
  - `packages/desktop/src/host/taskRealtimeBridge.ts` 与 `packages/desktop/test/taskRealtimeBridge.test.ts`；
    `packages/services/src/session/sessionRealtimePort.ts` 及 `@escode/services` 的
    `SessionRealtimePort` 导出（仅该 bridge 使用）。
  - `hostRemoteWorkspaceProxyState.ts` 的 `getTaskMeta` / `trackTaskReady` /
    `disposeTaskReadySubscription`（只服务于已删除的镜像分支）。
  - 同一轮补删的 Node Host 远程组装残留：`host/remoteConnectionServiceCollection.ts` 与
    `host/legacyRemoteWorkspaceRpcContract.ts`（`pnpm dep:refs` 显示 0 生产引用），以及
    `host/remotePromptAttachments.ts` + `host/promptAttachmentTransferService.ts`（只被各自测试导入，
    原由已删除的 `materializePromptAttachments` 选项提供），连同 3 个对应测试文件。
  - Main 侧 `main/providerProvisioningEnvironmentCoordinator.ts`（`ProviderProvisioningExecute` 的
    environment lane 协调器）及其测试：Main 发送端已于 S5-f-4 删除，`pnpm dep:refs` 显示只剩自身测试引用。
- 保留：`createReportingRemoteESCodeTaskService` 仍负责 task meta 记忆、`session-route-announce` 与
  `workspace_session_message_send_requested` 转发（main `taskRealtimeBus` 的会话消息路由依赖它们）；
  main 侧 `TaskRealtimeBus` 与 `HostMessageTypes.TaskRealtimeDeliver` 等协议面继续供 Rust Host 使用。
- 验证（2026-11-01，本机 Linux x64）：`pnpm typecheck` 通过；`pnpm lint` 0 error / 196 条存量 warning；
  `pnpm architecture:check --changed` OK；`pnpm verify:pre-push` 1568 files passed / 2 skipped、
  13849 tests passed / 27 skipped（三批删除合并后的最终树）；
  `pnpm check:escode-host` 通过。Node 本地 Host 冒烟：`ESCODE_RUST_HOST=0` 下
  `conversation-session-conversation-workspace.test.ts` 通过（artifact
  `desktop-e2e-20261101-003047-392`，退出日志 `killing host process (local-1)` 确认走 Node Host）。
