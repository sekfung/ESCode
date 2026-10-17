# Rust 阶段 2：`serial` 域（rust-unified-host.md 子 spec）

> 状态：切片交付中 · 2026-10-17。**1/4 片已交付**：端口枚举 + 轮询 + `onPorts`（`list` / `setWatching` / `onPorts`）。
> TS 真相源：`packages/services/src/serial/`（`serialService.ts` 359、`serialSession.ts` 386、
> `serialChunkBuffer.ts` 172、`serialLoop.ts` 80、`serialSignals.ts` 35、`serialRuntime.ts` 68 等，共 1763 行）。

## 1. 范围与分片

| 片                | 成员                                                       | 状态   |
| ----------------- | ---------------------------------------------------------- | ------ |
| **S1 枚举与轮询** | `list`、`setWatching`、`onPorts`                           | 已交付 |
| S2 会话           | `open`、`close`、`getSnapshot`、`listSessions`、`onStatus` | 待做   |
| S3 数据面         | `write`、`clear`、`onData`                                 | 待做   |
| S4 信号与循环     | `setSignals`、`startLoop`、`stopLoop`                      | 待做   |

## 2. 作用域（已登记的用户决定，2026-10-17）

TS 是**窗口级**：每窗口 4 个活动会话上限、`seq` 在窗口内统一分配（跨会话、跨关闭重开单调递增）。
Rust Host 是 App 级单进程，实现口径为**连接级 channel**（`Channels::with_scoped`，与 `window-controller` 同法）：
一个 Renderer 连接 ≈ 一个窗口，会话上限与 `seq` 都落在连接作用域内。

**登记条件**：手机远控等共享同一连接的多端会归入同一条连接域（TS 窗口级语义下它们本就是多个窗口）。

## 3. 端口枚举（S1）

- **Linux**：扫 `/sys/class/tty/`，只收**有 `device` 子目录**的条目（虚拟 tty/pty 没有），
  端口路径 = `/dev/<name>`；元数据按 `device/<field>` → `device/../<field>` 的顺序读取
  （USB 串口的 `idVendor`/`idProduct`/`manufacturer`/`serial` 在 USB 设备节点上，即接口的上一级）。
- **非 Linux**：本片只在 Linux 上实现；其它平台返回空列表并登记欠账（TS 走 `bindings-cpp` 的平台后端）。
- 枚举失败 → `SerialError("io", ...)`（与 TS 同 code），不是静默空列表。
- **测试接缝**：枚举器是**注入依赖**（`PortEnumerator` trait），测试用假实现；
  TS 的 `ESCODE_SERIAL_MOCK_PORTS` 环境接缝在 Rust 侧不需要（Host 不读 env，测试直接注入）。

## 4. 轮询与事件（S1）

- **间隔 1500ms**（TS `DEFAULT_POLL_INTERVAL_MS`）。
- **只在需要时轮询**：`watching === true` 或有 `disconnected` 会话时才有定时器（S1 只有前者）。
- `setWatching(true)` 会**清空 `lastPortsKey`** 并立即触发一次轮询（`schedulePoll(0)`），
  保证面板打开时立刻拿到一份端口列表。
- **事件去重**：按 `ports.map(p => p.path).join("\n")` 比较；只有变化时才广播 `onPorts`。
- **重入保护**：正在轮询时不再叠加定时器（`polling` 标志）。
- 轮询失败只记 debug 日志，不广播、不停止轮询。
- `setWatching(false)` 停止定时器与在飞轮询的续排。

## 5. 接口

| 方法                      | 行为                                                                                                                         |
| ------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| `list()`                  | 枚举当前端口，返回 `SerialPortInfo[]`（`path` + 可选 `manufacturer`/`serialNumber`/`vendorId`/`productId`）；枚举失败抛 `io` |
| `setWatching({watching})` | 切轮询开关；开启时清 dedupe 键并立即轮询一次                                                                                 |
| `onPorts`                 | 端口列表变化事件（`SerialPortInfo[]`，无参数）                                                                               |

## 6. 登记差异

1. **非 Linux 未实现**：Windows/macOS 的枚举（SetupAPI / IOKit）留给后续片；当前返回空列表。
   可见条件：在 Windows/macOS 上打开串口面板（S1 期间）。
2. **元数据 sysfs 路径**：`bindings-cpp` 的内部解析与本实现不完全相同（它有自己的 device 路径归一）；
   差异只在**元数据字段**上可见，`path` 集合一致。
3. **测试接缝形态**：TS 用 `ESCODE_SERIAL_MOCK_PORTS` 切 mock binding；Rust 用注入的
   `PortEnumerator`（生产实现读 sysfs）。对调用方不可见。
4. `ESCODE_SERIAL_MOCK_PORTS` 在 Rust Host 里**不生效**（Host 不读 env）；E2E 若要假端口，须由
   Main 经控制消息或测试注入。

## 7. 验收

- 单测：枚举器解析 sysfs 夹具目录（真实临时目录 + 假 `/sys` 树）——只收有 `device` 的子目录、
  USB 元数据回退到上一级、缺失字段不输出键；`setWatching(true)` 立即轮询并广播一次；
  同样的端口集合**不重复广播**；`setWatching(false)` 停止轮询；枚举失败不改 `lastPortsKey`、不广播。
- 端到端：`list` 返回端口；`setWatching(true)` 后 `onPorts` 收到一次；重复 `setWatching(true)` 不重复发。
- 负向验证：去掉 dedupe → 重复广播用例失败；去掉「立即轮询」→ 首次广播用例失败。

## 8. S1 交付记录（2026-10-17）

- **实现**：`host-services/src/serial/{mod,enumerate}.rs`。枚举器是 `PortEnumerator` trait
  （生产实现 `SysfsEnumerator`，测试注入假实现）；轮询是连接内的一条 tokio 任务，`shutdown()` 由
  channel 的 `with_close` 钩子调用。
- **通道**：`host-rpc/src/domains_serial.rs`，经 `Channels::with_scoped` 注册为**连接级 channel**
  （每连接一个服务实例，落实 §2 的作用域决定）。
- **测试**：`enumerate_tests` 3 例（USB 元数据回退到设备节点、虚拟 tty 与缺 `/dev` 节点跳过、
  缺 sys 根报错）+ `mod_tests` 5 例（`list` 透传、首次广播与去重、关 watch 停轮询、
  枚举失败不污染 dedupe、**长间隔 + 短超时**验证「立即轮询」）+ `tests/rpc_serial.rs` 端到端 1 例。
- **负向验证**（两项，均已恢复）：①去掉端口去重 → 「端口未变不应重复广播」失败；
  ②把首次轮询挪到 sleep 之后 → 「立即轮询」用例超时失败。
  **第二项第一版测不出来**：单元测试的轮询间隔是 10ms，「立即」与「等一个间隔」在 2s 超时下无差别；
  已补一个「间隔 5s + 超时 500ms」的用例专门钉住这条语义。
- **顺带修正**：`rpc_session.rs` 原先拿 `serial.list` 当「未迁移缺口」的样例，改为完全未迁的
  `bots.listBots`，并新增一条 `serial.list` 回数组的断言。
- **已知抖动**：`host-runtime/tests/instances.rs` 的 `an_unanswered_request_fails_with_a_classifiable_timeout`
  （零超时用例）在全量并行跑时偶发失败一次，单独与重跑全量均通过；与本片改动无关，未改它。
