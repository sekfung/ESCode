# Rust 阶段 2：窗口 Host 进程无关化（H2.3）

> 状态：窗口 attachment 已落地（2026-10-22，开关打开时生效）· 默认开关翻转仍被本文 §5 的 parity 缺口阻塞
> 父计划：`rust-unified-host.md` §阶段 2 / H2.3；相关：`rust-u2-host-resilience.md` §3.4
> 开关：`ESCODE_RUST_HOST`（`0`/`false` 关闭）；二进制覆盖：`ESCODE_RUST_HOST_BINARY`

## 1. 现状（2026-10-22 实测）

- `rustHostManager` 只在 Main 构造，生产代码只调用 `stop()`；**没有任何窗口 attachment 调用
  `attachLocalPort`**。手机远控删除后，原来的 Rust attachment 消费方也不存在了。
- 2026-10-22 已补 `desktopRustHostWindow.ts` 与 `desktopWindowLifecycle.ts` 的 Rust 分支：
  开关打开时窗口只建 attachment，不 spawn utility process；reload/close 只解绑。
- 因此 `ESCODE_RUST_HOST=1` **现在会真正改变窗口 Host**；默认仍关，原因见 §5。
- `ESCODE_RUST_HOST_BINARY` 覆盖已落地（E2E/自定义安装布局）。

## 2. 目标形态

```text
一个 App 进程
  └─ 一个 Rust Host 进程（App 级，Main 懒启动）
       ├─ workspace A 实例（embedded runtime）
       └─ workspace B 实例（embedded runtime）
            ▲
            │ @escode/rpc over MessagePort (desktop-continuous)
       ┌────┴────┐
       │ Window1 │  Window2
       └─────────┘
```

窗口关闭/reload 只解绑 attachment，不停止 Host；只有 App quit 调 `rustHostManager.stop()`。

## 3. 窗口 attachment 设计

### 3.1 创建

`dom-ready` 时若 `rustHostManager.enabled`：

1. `attachment = await rustHostManager.attachLocalPort(() => new MessageChannelMain(), "desktop-continuous")`；
2. 生成 `databaseStartupId`，向 renderer `postMessage(InternalChannels.ServicePort, {databaseStartupId}, [attachment.port])`；
3. 发一条 synthetic `DatabaseStartupState{phase:"ready", disk:[]}`（绑定同一 `startupId`）；
4. 不调用 `spawnHostProcess`、`broadcastHub.register`、`taskRealtimeBus.registerHost`、
   `registerHostProcess`。

### 3.2 reload / close

- reload：`oldAttachment.dispose()` → 重新 attach → 新 port + 新 synthetic ready。
  Host 与 runtime 实例保持存活。
- 窗口 close：只 `attachment.dispose()`；不调用 `rustHostManager.stop()`。
- App quit：`rustHostManager.stop()` + `disposeDesktopCuaPermissionService()` 进入既有退出屏障。

### 3.3 attachment 丢失

Host 意外退出时 `onLost` 触发：窗口收到 `failed/transport_closed` 启动状态，按
`rust-u2-host-resilience.md` §3.4 的“不原地重连”原则重新 attach 或在 renderer 展示
“正在重启/已放弃”。该 UI 面尚未实现（见 §5）。

## 4. 已经可用的能力

| 能力                                                              | 状态                                                            |
| ----------------------------------------------------------------- | --------------------------------------------------------------- |
| Renderer RPC（settings/file/tasks/session/agent V4/plugins/svn…） | 435/495 members Implemented                                     |
| 跨窗口 broadcast                                                  | Rust Host 内 `broadcast` channel                                |
| 任务流                                                            | Rust Host 内 `task_events` / `onDynamicStreamEvent`             |
| 无窗口 automation                                                 | Host 常驻 `automation_scheduler`                                |
| plugin/gen-ui sandbox 注册                                        | `mainRequest` → Main                                            |
| CUA 权限查询/重启                                                 | `mainRequest` → Main（`rust-u2-cua-permission.md`）             |
| 存量 TS 会话导入                                                  | embedded `LegacyImport::Implicit`（`rust-u2-legacy-import.md`） |

## 5. 默认翻转的阻塞项

以下能力仍通过 Node Host 的 `parentPort` 自定义消息实现，Rust Host 没有等价协议。
在这些缺口关闭前把 `ESCODE_RUST_HOST` 默认打开会让对应桌面功能静默降级：

1. **远程 workspace（SSH/WSL/Docker）**：`desktopRemoteSessions.ts` 依赖
   `AttachServicePort`/`BindRemoteWorkspaceContext`/`DisposeRemoteWorkspaceSession`/
   provider provisioning 等 Node Host 消息；Rust inventory 没有 remote 域。
2. **资源管理器 host 采样与进程行**：`registerHostProcess` 依赖 `pid` 与
   `ResourceUsageSnapshotRequest`；Rust Host 未上报实例子进程。
3. **网络捕获**：`HostMessageTypes.NetworkCapture` 无 Rust 处理。
4. **Agent 进程事件与运行任务计数**：`AgentProcessSpawned/Exited`、
   `AgentRunningTaskCountChanged` 无 Rust 处理；影响退出/更新安全与 CUA 指示。
5. **CUA 操作状态投影**：`HostResponseTypes.CuaOperationState` 无 Rust 处理。
6. **browser-use 执行请求**：`HostMessageTypes.BrowserUseExecute` 无 Rust 处理。
7. **数据库启动门**：Rust 模式目前只能由 Main 合成 `ready`，不反映真实 TS 迁移账本；
   需要明确的“Host 启动状态 → Main → renderer”协议。
8. **每窗口运行环境**：`runtimeProcessEnvPatch`（login-shell PATH 等）是窗口级，
   `rustHostManager.runtimeEnv` 是 Host 级且只求值一次；直接切换会让后续窗口继承首窗环境。
9. **Host 丢失 UX**：renderer 没有“正在重启/已放弃”页面；`failed` 后只能刷新。
10. **打包**：`electron-builder` 未把 `escode-host` 放进 `extraResources`；
    `defaultRustHostBinary` 在安装包内会指向 asar 内路径。

结论：**H2.3 的窗口 attachment 路径可以落地，但“默认开关翻转（M5）”必须等 §5 的
1–10 清零或明确产品降级。** 本文不把默认值改成 true，避免把未验证的降级带到默认桌面。

## 6. 验收（默认翻转前必须满足）

- 单测：Rust attachment 创建不发 `spawnHostProcess`、不注册 broadcast/taskRealtime/hostProcess；
  reload 复用 Host、close 只解绑；synthetic ready 被 `DatabaseStartupAdmission` 接受。
- 集成：两个窗口 attachment 到同一 Host，共享 workspace/task 投影；Host 重启后全部 attachment
  `onLost` 并重建。
- E2E（需要图形会话）：`ESCODE_RUST_HOST=1` + `ESCODE_RUST_HOST_BINARY` 启动；两窗口只有一个
  `escode-host` 进程、无 `escode-host-*` utility process；reload 保持同一 pid；关一窗另一窗健康；
  App quit 后 pid 消失。
- 远程 workspace、资源管理器、网络捕获、CUA 指示、browser-use、agent 进程计数逐项回归或登记降级。
