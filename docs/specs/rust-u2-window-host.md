# Rust 阶段 2：窗口 Host 进程无关化（H2.3）

> 状态：窗口 attachment 已落地（2026-10-22，开关打开时生效）· 默认开关翻转仍被本文 §5 的 parity 缺口阻塞
> 父计划：`rust-unified-host.md` §阶段 2 / H2.3；相关：`rust-u2-host-resilience.md` §3.4
> 开关：`ESCODE_RUST_HOST`（`0`/`false` 关闭）；二进制覆盖：`ESCODE_RUST_HOST_BINARY`

## 1. 现状（2026-10-22 实测）

- `rustHostManager` 只在 Main 构造，生产代码只调用 `stop()`；**没有任何窗口 attachment 调用
  `attachLocalPort`**。手机远控删除后，原来的 Rust attachment 消费方也不存在了。
- 2026-10-22 已补 `desktopRustHostWindow.ts` 与 `desktopWindowLifecycle.ts` 的 Rust 分支：
  开关打开时窗口只建 attachment，不 spawn utility process；reload/close 只解绑。
- 因此 `ESCODE_RUST_HOST` **现在决定窗口 Host**；2026-10-23 已完成四平台安装包验收并默认开启，
  显式 `0`/`false` 回退 Node Host（见 §6）。
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

## 5. parity 缺口（2026-10-23 已关闭）

| #   | 原缺口                           | 处理                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| --- | -------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | 远程 workspace（SSH/WSL/Docker） | **显式所有权**：本地窗口用 Rust Host；远程 workspace 仍用 per-window Node Host（TS 后端），首次 Connect/Bind 懒创建，写独立 `remoteWindowHostProcessMap`，不混入本地 map；日志与资源管理器可见。Rust `/ws` 永远不承载 remote scope。`desktopRemoteSessionsRustHostMode.test.ts` 验证懒创建；`ESCODE_RUST_HOST=1` 下的真实 SSH E2E 已验证完整连接链路（见 §5.1）。                                                                                                                                        |
| 2   | 资源管理器采样与 host 行         | Rust `process_sampler`（Host 自身 + 子进程树）+ `HostEvent.resource-usage-snapshot-result`；Main `registerExternalHostProcess` 把 Rust Host 作为外部 host 纳入采样与进程行。                                                                                                                                                                                                                                                                                                                             |
| 3   | 网络捕获                         | `escode-cli-net::capture` 250ms/100 条批量；model、webfetch、MCP SSE/OAuth HTTP、provider/API client 出口记录（`mcp_http` 只建 client，实际发送在 `mcp_sse`/`mcp_oauth_http`）；`HostEvent.network-capture-batch` → Main `ingestHostNetworkCapture`。                                                                                                                                                                                                                                                    |
| 4   | Agent 进程事件与运行任务计数     | `Instances::lifecycle` → `agent-process-spawned/exited`（child-link 有 pid；embedded 无 per-workspace 进程，Host 行即进程事实）；`TaskEventHub` 在 run start/terminal 发 `agent-running-task-count-changed`，Main 计入退出确认。                                                                                                                                                                                                                                                                         |
| 5   | CUA 操作状态                     | 实例帧 `computer-use/operation-event` → `HostEvent.cua-operation-state` → 既有 Windows 指示器；turn-completed/failed 发 inactive。                                                                                                                                                                                                                                                                                                                                                                       |
| 6   | browser-use 执行                 | `runtime_requests` 把 `interaction/browserExecute` 经 `MainRequest` 转给 Main 的 `runBrowserCommandOnView`；`browserList` 在 executor 缺失时按 TS 语义回空列表。                                                                                                                                                                                                                                                                                                                                         |
| 7   | 数据库启动门                     | `WorkspacePrepare` → `instances.get` → 真实 `startup/storageState`（含 checking/ready/failed）→ `WorkspaceStartupState` → `DatabaseStartupState`；Main 不再合成 ready。                                                                                                                                                                                                                                                                                                                                  |
| 8   | 每窗口运行环境                   | `WorkspacePrepare.runtimeEnvPatch` 与 `rpcListen.runtimeEnv` 分离；Host 按 identity 合并（patch 覆盖 Host env，identity 最后覆盖），只在实例首次创建时生效；后续窗口复用并记录差异。                                                                                                                                                                                                                                                                                                                     |
| 9   | Host 丢失 UX                     | manager `restarting` → 窗口启动门回 `checking` + renderer 恢复页；`running` → 新 startupId/新端口自动 reattach；`failed` → 可重试/退出的恢复页；Retry 清空重启预算并重建。                                                                                                                                                                                                                                                                                                                               |
| 10  | 打包                             | `prepare-rust-host.mjs` 构建并复制到 `bundled-tools/<platform>/escode-host`；electron-builder `extraResources → escode-host`；`defaultRustHostBinary` 解析 `resources/escode-host/<binary>`；已实际执行 `prepare-rust-host.mjs` 构建 release 二进制，并验证可执行、`shutdown` 退出 0；另执行 `electron-builder --linux dir` 生成 `dist/linux-unpacked/resources/escode-host/escode-host`，确认 extraResources 路径/可执行位/运行退出 0（该次打包在 afterPack 的 CUA koffi 资源校验处失败，与本项无关）。 |

### 5.1 远程 workspace 验收证据（2026-10-23）

本地环境：Xvfb `:99`、本地 sshd `127.0.0.1:2223`（独立用户 `escode-e2e`，workspace `/home/escode-e2e/ws-a`、`ws-b`）、linux-x64 remote assets。

```bash
ESCODE_RUST_HOST=1 \
ESCODE_RUST_HOST_BINARY=apps/escode-host/target/debug/escode-host \
ESCODE_E2E_SSH_HOST=127.0.0.1 ESCODE_E2E_SSH_PORT=2223 \
ESCODE_E2E_SSH_USERNAME=escode-e2e ESCODE_E2E_SSH_PASSWORD=... \
ESCODE_E2E_SSH_WORKSPACE_PATHS=/home/escode-e2e/ws-a,/home/escode-e2e/ws-b \
ESCODE_REMOTE_ASSET_PLATFORMS=linux-x64 \
NO_PROXY=localhost,127.0.0.1,::1 \
CHROMEDRIVER_PATH=/tmp/chromedriver/linux-146.0.7680.165/chromedriver-linux64/chromedriver \
wdio run wdio.conf.ts --spec ./test/e2e/conversation-session/conversation-session-ssh-remote-connection-p0.test.ts
```

结果：`1 passed`，artifact `desktop-e2e-20261023-060839-165`。用例覆盖错误密码失败（不提交 tab/逻辑 session、不泄露密码）→ 正确密码重试 → 远程目录选择（`ws-a`）→ remote workspace 打开；进程断言按 Rust 模式执行：本地窗口是 Rust Host，首次远程尝试懒创建一个 `escode-host-remote-1` 并在失败认证后保留复用。

ChromeDriver：Electron 41.10.7 的 Chromium `146.0.7680.216` 没有对应 ChromeDriver，使用主版本兼容的 `146.0.7680.165`，通过 `CHROMEDRIVER_PATH` 固定，避免每轮下载受网络影响。

该 E2E 暴露并验证了以下阻塞缺陷（均已修复）：

1. **Rust Host 启动过快**：Main 在 renderer 注册 `message` 监听前投递 `databaseStartupState`/ServicePort，页面永久停在 `root-startup-loading`。改为 Main 等待 renderer 的 `snapshot` 再投递，renderer 在业务根接管前按 250ms 重试 `snapshot`。
2. **桥接层帧不匹配**：Rust `/ws` 使用 `@escode/rpc` SocketProtocol 的 13 字节帧头，而窗口 MessagePort 使用无帧的 MessagePortProtocol；原实现按字节透传，`Initialize` 被 renderer 当作垃圾数据。`bridgeRustHostPort` 现在双向转帧。
3. **远程-only Node Host 未就绪**：`attachInitialServicePort:false` 时 `InitLocal` 缺少 MessagePort，Host 直接 return；且远程连接在 DB/services 就绪前下发 `ConnectRemoteWorkspace`，报 “Local Host services are not initialized.”。现在仍为 InitLocal 建立 base channel（Main 侧立即关闭），并以 Host 发布的 `databaseStartupState.ready` 作为连接前置条件。
4. **`file` channel 回归**：提交 `78d487f0` 删除营销链路时误删 `IFileService`/`IMediaPreviewService` 注册，远程 stdio server 不暴露 `file`（目录选择报 “Unknown channel: file”）。已恢复注册并重建 linux-x64 remote assets。
5. **prepared quit 不退出**：Rust Host 清理比 Node 模式多约 2s 后，ChromeDriver 已发起的 quit 重入被 Electron 忽略，窗口不再触发 `closed`，`deleteSession` 超时。现在在 host/agent 回收完成后显式销毁剩余窗口，并保留 3s 兜底退出。

该项仍是**产品边界**（远程 workspace 保留 per-window Node Host），不是 Rust 原生 remote parity；但边界行为已由上述可执行 E2E 验证。

**默认开关（2026-10-23 已翻转）**：`ESCODE_RUST_HOST` 未设置或为真值时使用 Rust Host；显式 `0`/`false` 回退 Node Host。依据是 CI run `39920171658`：win-x64、mac-arm64、mac-x64、linux-x64 四个 `installed` 作业的 “launch installed package with Rust Host” 步骤全部 success。

## 6. 验收（默认翻转前必须满足）

- 单测：Rust attachment 创建不发 `spawnHostProcess`、不注册 broadcast/taskRealtime/hostProcess；
  reload 复用 Host、close 只解绑；synthetic ready 被 `DatabaseStartupAdmission` 接受
  （`desktopRustHostWindow.test.ts`、`desktopWindowLifecycle.test.ts`）。
- 集成（真实 Host）：两个窗口 attachment 共用同一进程，Host 退出时两个 attachment 都收到
  `onLost`；Host 受限重启后新 attachment 可用（`rustHostResilience.test.ts` 的
  "serves multiple window attachments from one Host process" 与
  "restarts within the budget and attachments are rebuilt"）。
- E2E（Linux，2026-10-23 已通过）：`conversation-session-rust-host-window-lifecycle.test.ts`，
  artifact `desktop-e2e-20261023-064527-548`（显式 `ESCODE_RUST_HOST=1`）与
  `desktop-e2e-20261023-092550-025`（不设置任何 env，走翻转后的默认值）。本地窗口只由 1 个
  `escode-host-rust-host` 服务，无 `escode-host-local-*` utility host；renderer reload 后 Host pid
  不变。App quit 后 Host 退出由 `desktop-e2e` 退出屏障、`rustHostManager.stop()` 测试与 SSH E2E
  teardown 的 `[spawnHostProcess] host process ... exited` 覆盖。
- 远程 workspace：`conversation-session-ssh-remote-connection-p0.test.ts` 在
  `ESCODE_RUST_HOST=1` 下通过（§5.1）。
- 资源管理器、网络捕获、CUA 指示、browser-use、agent 进程计数：由 `rustHostHostEvents`、
  `rustHostCuaPermission`、`rustHostRuntimeCrash` 与对应 channel 测试覆盖；SSH E2E 回归了
  远程 host 行与 agent 进程注册。
- 默认翻转的 macOS/Windows 安装包 E2E 已接入且通过 CI：`.github/workflows/release.yml` 的
  `installed` 作业（win-x64 / mac-arm64 / mac-x64 / linux-x64）在真实安装后，再用
  `ESCODE_RUST_HOST=1 ESCODE_INSTALLED_EXPECT_RUST_HOST=1 node scripts/ci-install-desktop.mjs --launch-only`
  启动同一个包，断言包内 `escode-host` 启动、renderer attach，且没有 utility Node 窗口 Host。
  CI run `39920171658` 四个平台的该步骤全部 success；随后已把 `ESCODE_RUST_HOST` 默认翻转为开启
  （显式 `0`/`false` 回退 Node Host）。
