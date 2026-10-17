# Rust 阶段 2：Host 范围边界核实（rust-unified-host.md 子 spec）

> 状态：边界核实完成 · 2026-10-17
> 起因：库存盘点「46 通道 / 538 成员 / 228 已实现 / 310 待办」里，「疑似依赖 Electron/原生」的 43 个成员
> （serial / plugin-ui-bridge / gen-ui / plugin-ui-app-tools / plugin-ui-sampling / cua-pip-session / cua-permission）
> 此前只有 `cua-*` 被确认引用 electron，其余为按目录名的推测。本文件逐项读码核实，登记判定与依据，供迁移排序使用。
>
> 判定口径：一个成员「可迁」= **Host 进程能自足产生它的效果**（含经 `forward::runtime_call` 转发运行时）。
> 效果依赖 Electron Main、Renderer 或闭源 producer 的，一律不可自足，需显式登记落点。

## 1. 结论

| channel               | 成员 | 可迁 Host | 其余落点                  | 依据 |
| --------------------- | ---- | --------- | ------------------------- | ---- |
| `plugin-ui-bridge`    | 12   | 11        | 1 → Electron Main         | §3.1 |
| `plugin-ui-app-tools` | 4    | 4         | —                         | §4.1 |
| `plugin-ui-sampling`  | 2    | 2         | —                         | §4.2 |
| `gen-ui`              | 6    | 5         | 1 → Electron Main         | §3.2 |
| `serial`              | 14   | 14        | —（需 Rust 重建原生层）   | §4.3 |
| `cua-permission`      | 2    | 0         | 2 → Electron Main         | §2.1 |
| `cua-pip-session`     | 3    | 0         | 3 → 非缺口（随 producer） | §2.2 |
| **合计**              | 43   | 36        | 7                         |      |

修正后的待办结构：**310 = 已登记有意不迁 15 + 本文件不可迁 7 + 可迁 288**。两个 `prepareSandbox`（`plugin-ui-bridge`、
`gen-ui`）虽在「不可迁」列，但可通过拆分协议让**通道整体收口**（Host 负责 agent 段、Main 仍负责 register 段），见 §3。

### 1.1 「可迁」≠「现在可做」：运行时前置复核（2026-10-17 修正）

上表的「可迁 Host」只回答了**Host 侧代码能否自足**，没有回答**转发目标是否存在**。补上这一层复核后，
`plugin-ui-bridge`（11）/ `plugin-ui-app-tools`（4）/ `plugin-ui-sampling`（2）共 **17 个成员被上游阻塞**：

- `packages/services/src/plugin-ui-bridge/*` 的转发目标全是 **`mcp/ui*` 协议族**（`callTool` → `mcp/uiCallTool`、
  `sample` → `mcp/uiSampling`、`registerAppTools` → `mcp/uiRegisterAppTools` …）。
- **Rust runtime（`apps/escode-cli-rust`）一个 `mcp/ui*` 都没实现**：`crates/core/src/app/queries.rs:65` 注释明说
  「打开面板要整套 `mcp/ui*` 沙箱桥接，Rust runtime 尚未实现」，`plugins/listUiSurfaces` 目前如实回
  `{"surfaces": []}`（依据 `docs/specs/rust-v3.15.1-drift.md`）；全仓 grep `mcp/ui` 只命中该注释。
- 因此现在迁这 17 个成员，只会把 Host 的显式缺口（`NotImplemented`）换成运行时的
  `ESCODEProtocolClientError: Unsupported method: mcp/uiSampling`——**且无法与 TS 对拍验收**（TS 那侧走的是 TS CLI）。
- **前置工作包**：先让 Rust runtime 实现 `mcp/ui*`（`apps/escode-cli-rust`，以 `rust-v3.15.1-drift.md` 立项），
  再迁这三个域。注意 `plugin-ui-bridge` 的 `validateInstance` / `closeInstance` / `recycleInstance` 归属同一族，一并等。

`gen-ui` 的 5 个可迁成员**不受此影响**（`readDocument` + 四个状态成员是 Host 本地 FS + 锁 + watch，不经运行时）
——**已交付（2026-10-17）**，见 `rust-u2-gen-ui.md`。
`serial` 同样不受影响（Host 自建原生串口层）。

## 2. 不可迁项

### 2.1 `cua-permission`（2：`getStatus`、`restartHelper`）— 登记为**缺口**，落点 Electron Main

**判定**：不可迁。它是唯一真正 Electron-bound 的域，三处硬依赖：

1. **Helper 的 launcher 必须是签名的 Electron 主进程**。macOS 下 Main 把**自身 pid** 注入 host env 为 `ESCODE_CUA_LAUNCHER_PID`，
   注释明说 forked host utilityProcess 的签名身份是嵌套 Electron helper（非 `dev.escode.app`），peer 校验会失败
   （`packages/desktop/src/main/desktopHostProcess.ts:276-282`）。Rust Host 没有这个签名身份可供借出。
2. **Windows Helper 需要 Electron/Node 运行时 + 按 `electronVersion` 锁定的 ABI addon**：`command: process.execPath`
   配 `ELECTRON_RUN_AS_NODE=1`（`packages/services/src/cua-permission-broker/windowsCuaDevRuntime.ts:141-146`），
   manifest 校验 `electronVersion: process.versions.electron`（同文件 `:189-191`）。
3. **`restartHelper` 必经闭源 producer 的 resolver**（`packages/services/src/node.ts:1793-1812`）。

`getStatus` 的读取支路（socket ping + `permission_status` JSON-RPC + `/usr/bin/open` 冷启）技术上可用 Rust 复刻，
但两成员共享同一 Helper 生命周期 owner，拆开无意义，整体判不可迁。

**为什么是缺口而不是非缺口**：`cua-permission` **在 Renderer 可达面上**——`IServiceAccessor`（`packages/services/src/accessor.ts:71`）
与 `RemoteServiceAccess`（`packages/client/src/remoteServiceAccess.ts:76,156`）都暴露它，UI 有真实消费者
（`packages/ui/src/settings/ComputerUseSection.tsx`、`hooks/useCuaPermissionStatus.ts`）。该 hook 对「服务缺失」的
处理是**降级显示「未知」**（`useCuaPermissionStatus.ts:38-40`，注释写明「远端 host 无 CUA」），所以公开构建（占位包
fail-closed）下无可见损失；但**装了闭源 producer 的私有构建**切到 Rust Host 后，Computer Use 设置面会退化为「未知」。

**落点建议**：Electron Main 已经持有这条链的其余部分——helper 安装器 `desktopCuaHelperInstaller.ts:1-12`
（`app` + canonical installer）与权限引导 UI `desktopCuaPermissionIpc.ts:5`（`app, BrowserWindow, ipcMain, nativeImage, screen`）。
因此正确的收口是**由 Main 直接提供这两个方法**（而非 Host 转发给 Main）。做之前需要先定 Main↔Host 的能力分工口径，
故本文件只登记判定，不在此处开工。

### 2.2 `cua-pip-session`（3：`dispose`、`publishFocus`、`publishLifecycle`）— 登记为非缺口

**判定**：自身代码可迁（纯 socket + 定时器 + 重放记账，无 Electron/addon/DOM），**依赖链不可迁**：唯一出口
`createPipSessionClient` 来自 `@escode/escode-cua/pip-session/node`，而本 checkout 是占位包
（`packages/escode-cua/package.json` 的 `escodeCuaPlaceholder: true`，`pip-session-node.js` 的 `createPipSessionClient`
恒回 `{enabled: false}`）；传输目标是 producer Helper 的 unix socket / 命名管道。

**非缺口依据**：**无任何 Renderer 调用方**——不在 `IServiceAccessor`、不在 `RemoteServiceAccess`，`packages/ui` /
`packages/client` / `packages/web` 全仓零引用（grep 无命中）。它只是 Host 内部投递链（`node.ts:1961` 的
`publishLifecycle` 由 agent service 事件驱动）。Rust Host 不服务它 = 无可见差异。

## 3. 部分可迁项：`prepareSandbox` 是唯一不可迁成员

两个域的 `prepareSandbox` 都**不是** DOM 沙箱操作，而是**复合调用**：Host 侧做准备（校验 / appIdentity 派生 /
初始状态内嵌），最后一跳交给 Electron Main 注册真实沙箱。Main 注册返回的句柄携带 `shellUrl` / `partition`
（Electron partition + guest `WebContents` + `MessageChannelMain` 的产物），Rust Host 无法自足产出。

### 3.1 `plugin-ui-bridge`（12 → 11 可迁）

Host 侧实现是**依赖注入壳**，`packages/services/src/plugin-ui-bridge/` 整层不 import electron、不碰 DOM：

- **11 个可迁**（纯 agent 运行时转发，Rust 侧有现成 `forward::runtime_call`，见 `host-rpc/src/forward.rs`）：
  `callTool`、`cancelToolCall`、`readResource`、`listResources`、`listResourceTemplates`、`subscribeResource`、
  `unsubscribeResource`、`listSurfaces`、`validateInstance`、`recycleInstance`、`closeInstance`。
  **注意**：`validateInstance` / `closeInstance` / `recycleInstance` 操作的是 **agent 签发的内存凭证**
  （`McpAppInstance` = `runtimeId/generation/appIdentity/token`），落到 `mcp/uiValidateInstance` / `mcp/uiCloseInstance`
  （`packages/services/src/escode-agent/escodeAgentService.ts:4404-4446`），**不是** Renderer 的 DOM 沙箱——
  这点与按方法名的直觉相反，登记以免后续误判。
  Host 侧另有 `instanceAccounts.ts`（`active` Map + `revision` 代际失效）与之配套，同样是纯内存态。
- **1 个不可迁**：`prepareSandbox`——其 `registerSandbox` 依赖经 `parentPort` 注入
  （`packages/services/src/node.ts:1414`，Desktop 侧桥在 `packages/desktop/src/host/pluginSandbox/index.ts`，
  Main 侧收口 `packages/desktop/src/main/index.ts:1929-1934` 的 `getPluginSandboxHost().registerFromHost`）。

### 3.2 `gen-ui`（6 → 5 可迁）

`packages/services/src/gen-ui/adapters/service.ts` 的契约注释已写明分工
（`packages/services/src/gen-ui/contract.ts:18`）：「readDocument runs on the executor Host. State and sandbox
methods run on desktop base services.」

- **5 个可迁**（纯 Node FS + 跨进程锁 + watch，Rust 可自足）：`readDocument`（受限读取 + 符号链接拒绝 +
  5MB 上限 + 拒整文档只收 fragment）、`getState` / `setState` / `listState` / `onStateChanged`
  （`getAppConfigDir()/gen-ui-state/<sha256(scopeKey)>/<sha256(path)>.json`，原子替换 + 文件锁 + watch 投影）。
  迁移需逐位对齐 scope-hash 目录约定与锁语义。
- **1 个不可迁**：`prepareSandbox`——壳 HTML 拼装可搬，但生效步骤同 §3.1（Main 的 `registerFromHost`）。

**连带约束**：`readDocument` 走**执行端**、state + sandbox 走 **base services**（`packages/ui/src/hooks/useGenUi.tsx:16,22`），
迁移时必须保留远端执行端 Host（`packages/server/src/entry-stdio.ts` → `createLocalServices`）的 6 个成员注册，
否则远程 workspace 的 `readDocument` 断链。

## 4. 全可迁项

### 4.1 `plugin-ui-app-tools`（4）

零状态透传（契约注释：登记表、暴露集合与待执行调用由 agent 唯一持有）。四个成员都是一行转发
（`plugin-ui-bridge/pluginUiAppToolsService.ts:33-48`）→ `mcp/ui{RegisterAppTools,UnregisterAppTools,ClaimAppToolCall,ResolveAppToolCall}`。
**语义注意**：缺依赖时三个方法抛 `unavailable`、`unregisterAppTools` 回 `{removed: 0}`——这是「运行时是否存在」的语义，
Rust 侧用 presence 一致表达即可。

### 4.2 `plugin-ui-sampling`（2）

零状态透传（契约注释：「无状态透传；Agent 持有实例凭证、并发限额和唯一调用终态」）→ `mcp/uiSampling` /
`mcp/uiCancelSampling`。**语义注意**：TS 侧有 `MCP_APPS_SAMPLING_TRANSPORT_TIMEOUT_MS` 超时后自动补发 cancel 的兜底
（`escodeAgentService.ts:4476-4487`），且用 `existing-only` 客户端模式（Rust 侧对应 `ESCODE_AGENT_RUNTIME_UNAVAILABLE`）。

### 4.3 `serial`（14）— 可迁，但阻塞是原生层重建不是 Electron

**判定修正**：此前归类「疑似依赖 Electron/原生」中「原生」的部分成立，但**可迁**——Rust 有等价能力。
零 Electron 引用（该目录无 `electron` / `app` / `BrowserWindow` / `webContents` / DOM 命中）。

- **依赖**：`@serialport/bindings-cpp`（N-API addon，`packages/services/src/serial/serialRuntime.ts:1,24-27` 的
  `await import(...)` → `autoDetect()`）+ `@serialport/stream` 的 `SerialPortStream`。Rust Host 不能加载 N-API addon，
  需用 Rust 串口库重建：枚举（含 manufacturer/serialNumber/vendorId/productId）、open 配置、DTR/RTS 与信号脉冲时序、
  环形缓冲与 10ms 合并、热插拔轮询与自动重连、`nativeUnavailable` 错误语义、`ESCODE_SERIAL_MOCK_PORTS` 等价测试接缝。
- **作用域已定（2026-10-17，用户决定）**：**每连接（≈每窗口）一个串口域**。TS 是窗口级
  （`SERIAL_MAX_ACTIVE_SESSIONS = 4` 每窗口，`packages/services/src/serial/serial.ts:108`；`seq` 在窗口内统一分配、
  跨会话跨重开单调递增，`docs/specs/serial-port-debugger-phase3.md:10,14,22`），而 Rust Host 是 App 级单进程。
  实现口径：用**连接级 channel**（`Channels::with_scoped`，与 `window-controller` 同法）承载串口域，
  一个 Renderer 连接 ≈ 一个窗口，4 个会话上限与 `seq` 的单调性都落在连接作用域内。
  **登记条件**：手机远控等共享同一连接的多端会归入同一条连接域（TS 窗口级语义下它们本就是多个窗口）；
  迁移时在 serial 子 spec 里登记这条可见差异。
- **收益明确**：UI 入口只在桌面端（`packages/desktop/src/renderer/src/desktopPlatform.ts` 是唯一
  `supportsSerialPort: true`），web/手机不可达；但不迁则切 Rust Host 后桌面串口面板直接拿 `NotImplemented`。

## 5. 对迁移顺序的影响

1. **新增 6 个低成本成员**（原盘点未列入快赢批）：`plugin-ui-app-tools`（4）+ `plugin-ui-sampling`（2）是零状态
   运行时转发，**但按 §1.1 被运行时 `mcp/ui*` 阻塞**——先做运行时前置，不是现在开工。
2. **`gen-ui` 的 5 个成员**与已迁的 skills / memory 同属「纯文件配置」模式，可并入该批（Host 本地，不受 §1.1 影响）。
3. **`plugin-ui-bridge` 的 11 个成员**有两重前置：① 运行时 `mcp/ui*`（§1.1）；② `prepareSandbox` 的协议拆分
   （Host 负责 agent 段、Main 仍负责 register 段）——两者都做完才能收口通道。
4. **`serial`（14）从「疑似不迁」上调为「可迁但需独立立项」**：它是本批唯一需要新建 Rust 原生能力（串口）
   的域，成本显著高于其他，且带一个必须先定的作用域决策，不适合塞进快赢批。
5. **`cua-*`（5）移出迁移范围**：`cua-pip-session`（3）登记非缺口、不再计入待办；`cua-permission`（2）登记为
   缺口，落点 Main，需在阶段 3 切默认前定分工口径。
6. 因此可迁待办的准确口径是 **288**，而非盘点的 310；其中 7 个成员已在本文件定型去向。

## 6. 未核实项

- `plugins`（7）、`hooks`（3）、`git-checkpoint`（4）等「纯文件配置」批的难易沿用盘点判断（同类模式已在
  skills / memory 验证），本文件未逐行复核；file 行数抽查吻合（`pluginsService.ts` 86 行、
  `gitCheckpointService.ts` 111 行）。
- `conversation-share`（9）、`bots`（32）、`settings-sync` / `skill-sync` / `mcp-sync` / `plugin-sync`（26）
  未做边界核实，其归类仍是推测。
