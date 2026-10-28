# 远程 workspace 统一到 Source Server（Rust Host）

> 状态：方向已确认，未实施 · 2026-10-28
> 关联：[upstream-v3.16.2-sync.md](upstream-v3.16.2-sync.md)、[rust-unified-host.md](rust-unified-host.md)、[rust-u2-window-host.md](rust-u2-window-host.md)、[rust-u2-host-skeleton.md](rust-u2-host-skeleton.md)
> 上游参考：`shared/src/source-server/*`、`desktop/src/host/sourceServer*`、`server/src/source-server/*`、`docs/architecture/r3-source-server-remote-architecture.md`

## 1. 决策

2026-10-28 用户确认（上游 v3.16.2 同步 Q3）：

1. **本轮（v3.16.2 同步）**只跟进 Source Server 的协议、兼容字段与只读展示；远程 workspace 继续由每窗口 Node Host 承载，Rust Host 不承载远程 scope。
2. **后续阶段**把远程 workspace 统一到 Source Server，并落到 Rust Host；目标状态下 Rust Host 是远程连接与实例表的唯一 owner，最终删除 per-window Node Host 远程路径。
3. 迁移不是“加超时/加兜底”，而是所有权转移：同一时刻只能有一个 Host 服务同一个远程 workspace。
4. 不恢复已删除的手机远控、会话分享、Bots 等产品面；只保留协议兼容字段和桌面端 `desktop-continuous` 语义。

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

| 阶段 | 内容                                                               | 退出条件                                           |
| ---- | ------------------------------------------------------------------ | -------------------------------------------------- |
| S1   | 协议/兼容字段、只读展示（本轮 v3.16.2 同步 W1/W3）                 | schema 测试与类型检查通过；远程仍由 Node Host 服务 |
| S2   | Rust Host 的 SourceServerRegistry、Connection Profile、状态/展示名 | 本地不受影响；远程只读面可用                       |
| S3   | 目录绑定、runtime 实例、continuous 投递                            | 首连/断线/重启恢复 E2E                             |
| S4   | 更新、迁移、恢复策略                                               | 更新失败显式报错；迁移报告可读                     |
| S5   | 删除 per-window Node Host 远程路径                                 | 三平台 E2E 全绿；无静默回退                        |

## 8. 非目标

- 不恢复手机 Web `/remote`、会话分享、Bots、off-peak、遥测上报。
- 不在本轮把远程迁到 Rust Host。
- 不新增上游之外的协议语义，不为同步问题加超时兜底。
- 不改变本地 workspace 的默认 Rust Host 行为。
