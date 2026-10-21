# Rust 阶段 2：`serial` 域（rust-unified-host.md 子 spec）

> 状态：已收口 · 2026-10-17。**4/4 片已交付**：S1 枚举/轮询/`onPorts`；S2 会话（含自动重连）；
> S3 数据面（`write` / `clear` / `onData`，含 AsyncFd 读路径与 RX 合并）；S4 信号（DTR/RTS + 复位脉冲）
> 与定时循环（`startLoop` / `stopLoop`）。`serial` 14/14。
> TS 真相源：`packages/services/src/serial/`（`serialService.ts` 359、`serialSession.ts` 386、
> `serialChunkBuffer.ts` 172、`serialLoop.ts` 80、`serialSignals.ts` 35、`serialRuntime.ts` 68 等，共 1763 行）。

## 1. 范围与分片

| 片                | 成员                                                       | 状态   |
| ----------------- | ---------------------------------------------------------- | ------ |
| **S1 枚举与轮询** | `list`、`setWatching`、`onPorts`                           | 已交付 |
| **S2 会话**       | `open`、`close`、`getSnapshot`、`listSessions`、`onStatus` | 已交付 |
| **S3 数据面**     | `write`、`clear`、`onData`                                 | 已交付 |
| **S4 信号与循环** | `setSignals`、`startLoop`、`stopLoop`                      | 已交付 |

## 2. 作用域（已登记的用户决定，2026-10-17）

TS 是**窗口级**：每窗口 4 个活动会话上限、`seq` 在窗口内统一分配（跨会话、跨关闭重开单调递增）。
Rust Host 是 App 级单进程，实现口径为**连接级 channel**（`Channels::with_scoped`，与 `window-controller` 同法）：
一个 Renderer 连接 ≈ 一个窗口，会话上限与 `seq` 都落在连接作用域内。

**登记条件**：远程 workspace 等共享同一连接的多端会归入同一条连接域（TS 窗口级语义下它们本就是多个窗口）。

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
- **顺带修正**：`rpc_session.rs` 新增一条 `serial.list` 回数组的断言。
- **已知抖动**：`host-runtime/tests/instances.rs` 的 `an_unanswered_request_fails_with_a_classifiable_timeout`
  （零超时用例）在全量并行跑时偶发失败一次，单独与重跑全量均通过；与本片改动无关，未改它。

## 9. S2 会话交付记录（2026-10-17）

### 9.1 状态机

`closed → opening → open → closing → closed`，以及 `open → disconnected →（设备回来）→ opening → open`。
`disconnected`（等设备回来）与 `error`（打开失败、需用户重试）是两种不同的非活动态。
`isActive = state !== "closed"`，因此**4 会话上限与 `listSessions` 都把 `error` 计为活动**。

状态事件 `onStatus` 载荷 `{path, state, signals, config?, error?}`；`signals` 恒在（S4 才真正驱动，
当前恒为默认 `{dtr:true, rts:true}`）。**构造会话时不发状态**（TS 同）——首个事件是 `opening`。

### 9.2 传输层（`transport.rs`）

- 生产实现：Linux `termios` —— `open(O_RDWR|O_NOCTTY|O_NONBLOCK)` → `tcgetattr` → `cfmakeraw` →
  `cfsetspeed` → 数据位/校验/停止位/RTS-CTS 配置 → `tcsetattr`；关闭即 `close(fd)`。
- 配置校验（`SerialConfig::from_json`）：`baudRate > 0`、`dataBits ∈ 5..8`、`parity` 五项、
  `stopBits ∈ {1,1.5,2}`；多个问题按 `baudRate, dataBits, parity, stopBits` 顺序拼接
  `Invalid serial config: …`。
- 打开错误映射（`from_message`）：`access denied|resource busy|cannot lock port|port is locked|ebusy` → `busy`；
  `permission denied|eacces` → `denied`；`file not found|no such file|does not exist|enoent` → `notFound`；
  `invalid argument|einval` → `invalidConfig`；其余 `io`。
- RPC 错误形状：`name = "SerialError"` + 稳定 `code`。

### 9.3 自动重连（用户决定：算在 S2 里）

- **断开检测改用轮询**：S1 的轮询已经在枚举端口；`open` 中的会话其路径从枚举里消失即判断开。
  TS 由驱动的 close 事件**即时**发现；Rust 最多晚一个轮询间隔（1500ms）。**登记差异**。
- `autoReconnect=true` → 进 `disconnected`（`shouldPoll` 会因此保持轮询）；设备回来时轮询触发 `reconnect()`。
- `autoReconnect=false` → 直接 `closed`，设备回来也不会自己重开。
- 用户主动 `open` 会重置信号（S4 的语义）；`reconnect` 不重置。

### 9.4 其他实现要点

- `open` 已在打开同一配置时直接返回；状态为 `disconnected` 时先收口再开（TS `openNow` 同）。
- `close` 幂等；未知路径的 `close` 是空操作。
- 保留 `MAX_RETAINED_SESSIONS = 8`：超出时按 `lastChangedAt` 淘汰最早的已关闭会话（`getSnapshot` 之后回 closed 空快照）。
- `getSnapshot`：已知会话回真实状态；未知路径回 `closed` 空快照 + 连接内 seq。
  **本片 `chunks` 恒为空、`stats` 恒为零**——数据面是 S3。

### 9.5 登记差异

1. **非标准波特率被拒绝**（TS 用 `termios2`/`BOTHER` 支持任意整数）：标准集覆盖 9600…4000000。
2. **非 Linux 返回 `nativeUnavailable`**（TS 走 `bindings-cpp` 平台后端）。
3. **断开检测来源**：轮询（最多晚一个间隔）vs 驱动 close 事件。
4. `1.5` 停止位要求 5 位数据（POSIX 限制）；TS 的驱动同样受此约束，但错误文案不同。
5. `chunks`/`stats` 在本片为空（S3 接入）。

### 9.6 验收

- 单测 16 例新增：`transport_tests` 2（配置往返与校验错误拼接、错误码映射）、`session_tests` 3
  （open/close 状态与事件序列、打开失败落 error、断开→重连与 autoReconnect=false 的行为差）、
  `mod_tests` 5（快照与 listSessions、第 5 个活动会话被拒、轮询检测断开并自动重连、
  无 autoReconnect 直接 closed、状态事件广播）。
- 端到端 1 例（无硬件）：非法配置 → `SerialError` + `invalidConfig`；合法配置但设备不存在 →
  `notFound`；`onStatus` 落 `opening → error`；未知路径快照 closed；`error` 会话计入 `listSessions`；
  关闭后清空。
- **负向验证**（两项，均已恢复）：①去掉 4 会话上限 → 第 5 个被接受，用例失败；
  ②断开时忽略 `autoReconnect` → 单测与轮询重连用例双双失败。

## 10. 收发缓冲（S3 前置件，2026-10-17）

`buffer.rs` 是**纯数据结构**，可确定性测试：

- **seq 由连接分配**（调用方传 `allocate_seq`）：跨会话、跨关闭重开单调递增；同一缓冲内的 seq 可以不连续。
  `clear()` 不回退 seq。
- **按字节上限淘汰**：从最旧开始，且**至少保留最新一条**（否则单条超过上限时 seq 会原地踏步）。
  `evicted_through` 记录已淘汰的最大 seq——游标早于它说明中间有数据缺失。
- **统计**：`rxBytes`/`txBytes` 累计；`clear()` 一并归零。
- **`sessionId` 只在 `source === "agent"` 时保留**（`user` 写入不带）。
- `onData` 载荷 = `{...chunk, path}`（`Chunk::to_json`）。

**RX 合并的定时不在本模块**：TS 在缓冲内部用 `setTimeout` 做 debounce；Rust 侧把 debounce 放在会话层
（尾部静默窗口），缓冲只收已成段的数据——这样缓冲可以脱离 tokio 计时器单测。

### 10.1 S3 剩余部分（下一步）

1. **传输层读路径**（用户已定：`AsyncFd` + epoll）：`SerialPort::spawn_reader` 用 `AsyncFd` 等
   `POLLIN` 后非阻塞读，读到字节经回调上送；关闭时先 abort reader 再 `close(fd)`。
2. **传输层写路径**：非阻塞 fd 的 `write` 在 EAGAIN 时短重试（上限 5s），成功后 flush RX 再入缓冲
   （保证回显出现在设备响应之前）。
3. **会话接线**：持 `ChunkBuffer` + 尾部静默窗口的 debounce（`window` 默认 50ms、`maxBytes` 上限合并）；
   `write` 校验 64KB 上限与空写；`clear`；`snapshot` 返回真实 chunks/stats。
4. **服务与 RPC**：`write` 回 `{seq}`、`clear`、`onData` 事件（按 path 区分）。
5. **验收**：单测（空写返回当前 seq、64KB 上限、EAGAIN 重试、debounce 的尾部窗口语义、agent/user 的
   `sessionId` 差）+ 端到端（PTY 对：写 master → 从 slave 读到；写 slave → 面板收到 `onData`）。

## 11. S3 数据面交付记录（2026-10-17）

- **读路径**（用户决定：AsyncFd + epoll）：`SerialPort::spawn_reader` 用 `dup` 出来的 fd 建
  `AsyncFd`（`unsafe { AsyncFd::register }`，tokio 1.53 已弃用 `new`），`readable()` 就绪后非阻塞读；
  EAGAIN 时 `clear_ready()` 继续。关闭时**先 abort 读任务再 close(fd)**（否则 epoll 会在已关闭的 fd 上触发）。
- **写路径**：非阻塞 fd 上 `write` 遇 EAGAIN 以 1ms 间隔重试，上限 5s；成功后返回本次 TX 的 seq。
  **先记 TX 再真正写出**（TS 同序）：保证 TX 的 seq 先于设备回显。
- **RX 合并**（`data.rs`）：字节进 pending，**尾部静默窗口**（10ms，`COALESCE_WINDOW`）后才成段；
  pending 超过 4096 字节立即成段。缓冲上限 1 MiB（`BUFFER_LIMIT_BYTES`），写上限 64 KiB。
- **事件**：`onData` 载荷 `{...chunk, path}`；TX 与 RX 都发（TS 同）。
- **RPC 字节形态**：对象里的 `Uint8Array` 由 `@escode/rpc` 编码成
  `{__escode_rpc_nested_uint8array_v1: true, base64}`；`domains_serial.rs` 的 `param_bytes` 解码它，
  同时接受纯数字数组（测试/JSON 直发）。

**登记差异**：`write` 的 EAGAIN 重试用固定 1ms 间隔（TS 由驱动做异步写+`drain`），
可见差异只在「写缓冲长期打满」的极端情况下（延迟略有不同，不改变顺序语义）。

**验收**：单测 6 例新增（`data_tests` 5：TX seq 与 stats、尾部窗口合并、超限立即成段、
TX 前先 flush RX、clear 不回退 seq；`service_tests` 2：写→TX 事件→设备字节→RX 事件→快照统计、
空写回当前 seq、64KB 拒绝、clear 不动状态、未打开路径 `notOpen`）。
负向验证两项（去掉 64KB 上限 / RX 立即成段），均已恢复。

## 12. S4 信号与循环（设计，2026-10-17）

TS 真相源：`serialSignals.ts`（35 行，脉冲时序）、`serialLoop.ts`（80 行，循环发送）、
`serialSession.ts` 的 `setSignals`/`startLoop`/`stopLoop`/`setStatus` 调停。

### 12.1 `setSignals`

- 服务层：未知路径 → `SerialError notOpen`（"Serial port is not open"）；已知会话但当前非 `open`
  同样 `notOpen`。
- 与 `open`/`close`/`reconnect` **同一会话队列串行**：脉冲进行中的 `setSignals` 排在脉冲结束之后，
  不会交错改线；同一时刻只有一个脉冲。
- **RTS 与流控**：参数里显式带 `rts` 且会话配置 `rtscts === true` → `invalidInput`
  （"RTS is controlled by RTS/CTS flow control"）。**脉冲不做这个检查**（TS 只检查显式 `rts` 字段）。
- 非脉冲：`dtr ?? previous.dtr`、`rts ?? previous.rts`。
- 脉冲（`serialPulseSteps`，最后一步之后**恢复脉冲前状态**）：
  - **ESP32**：`{dtr:false,rts:true}` 保持 100ms → `{dtr:true,rts:false}` 保持 50ms →
    `{dtr:false,rts:false}`（hold 0）；
  - **Arduino**：`{dtr:true,rts:previous.rts}` 保持 100ms → `{dtr:false,rts:previous.rts}`（hold 0）。
- 生效后广播一次 `onStatus`（`signals` 已更新），返回 `{dtr, rts}`。
- **打开语义**：用户 `open` 把信号重置为默认 `{dtr:true,rts:true}`；`reconnect` 保留断开前状态，
  打开后尝试恢复（恢复失败不改变 `open` 状态——TS 记 warn）。

### 12.2 DTR/RTS 原生实现

- Linux：`TIOCMGET` 读当前 modem bits，置/清 `TIOCM_DTR` / `TIOCM_RTS`，再 `TIOCMSET` 写回
  （保留其它位）。在 `spawn_blocking` 里执行；失败 → `io`。
- fd 改成原子 `-1` 收口：`close` 只关一次，重复关闭是空操作；`write`/`set_signals` 在已关闭的 fd 上
  返回 `notOpen`，不会误写复用的 fd。`NativePort` 的 `Drop` 兜底调用 `close`。
- 测试注入假传输，记录每次 `set_signals` 的 `(dtr,rts)` 序列。

### 12.3 `startLoop` / `stopLoop`

- `startLoop`：未知路径或非 `open`/无端口句柄 → `notOpen`；参数校验
  `intervalMs` 为整数且 ≥ 10、`bytes` 非空且 ≤ 64 KiB、`count` 缺省或为整数 ≥ 1，
  否则 `invalidInput`（"Invalid loop parameters"）。
- **立即发送第一次**，之后按 `intervalMs` 定时；上一笔写入未完成时跳过当次 tick，不排队积压。
- 每次发送等同一次 `source="user"` 写入（TX 进缓冲、分配 seq）。
- 每个会话同时只有一个任务：再次 `startLoop` **静默替换**旧任务（旧任务不再产生事件）。
- 进度事件：启动时立即一条；之后距上次进度 ≥ 250ms 才再发一条。
- 达到 `count`、写入失败、`stopLoop`、或状态离开 `open`（关闭/断开/出错）时任务结束；
  结束后的状态事件**不带 `loop`**。
- `stopLoop`：没有进行中的循环时无事件（幂等）；有循环时停止并广播不带 `loop` 的状态。
- Agent 串口工具没有循环发送入口（TS 同），本片不新增 Agent 侧 RPC。

### 12.4 状态载荷

`onStatus` 直接广播完整状态对象 `{state, path, signals, config?, error?, loop?}`；
`loop = {intervalMs, count?, sent}` 只在循环进行中存在（S2 起 `signals` 恒在但恒为默认，S4 起为真实值）。
此前 `mod.rs` 重组事件载荷时把缺失的 `config`/`error` 写成 `null`，与 TS 的「缺键」不同；本片改为直接广播。

### 12.5 连接关闭收口

`SerialService::shutdown` 从同步改为异步：置 disposed、停轮询、**关闭所有会话**（TS
`disposeAllAndWait`）并停掉循环任务，再清空监听。否则循环任务持有 `Arc<SerialSession>`，
连接关闭后仍会继续写；端口 fd 也只在进程退出时才释放。

### 12.6 验收

- 单测：脉冲步骤序列与保持时间（ESP32/Arduino、恢复原状态）；`setSignals` 的 rtscts 拒绝、
  非 open 拒绝、返回值与状态事件；`startLoop` 参数校验、立即首发送、按 count 结束、
  重启用新任务替换旧任务、`stopLoop` 幂等、关闭/断开自动停；状态载荷含 `signals`/`loop`。
- 端到端：未知路径的三个方法分别回 `notOpen`/`notOpen`/无操作，错误形状带 `code`。
- 负向验证：①去掉 250ms 进度节流 → 高频进度用例失败；②去掉「状态离开 open 自动停」→
  关闭后仍收到 loop 写入，用例失败。

## 13. S4 交付记录（2026-10-17）

- **实现**：
  - `serial/signals.rs`：`Signals` / `DEFAULT_SIGNALS` / `Pulse` / `pulse_steps`（ESP32 100ms+50ms、
    Arduino 100ms，最后一步后由会话恢复脉冲前状态）；`Pulse::from_name` 复刻 TS 的
    `if (pulse === "esp32") … else …`（空串是 falsy → 无脉冲）。
  - `serial/loop_runner.rs`：`LoopState`（generation/间隔/次数/已发/stop/notify）+ `spawn`。
    首个 tick 立即发送；写入未完成时不叠加（固定间隔 + `MissedTickBehavior::Skip`）；
    `stop` 用 `Notify` 的 permit 语义，长间隔也能立即退出；进度事件 250ms 节流；
    达到次数/写入失败调用 `on_finished`。任务只依赖注入的写函数与回调，可脱离会话单测。
  - `serial/session_status.rs`：状态投影抽离，`{state, path, signals, config?, error?, loop?}`，
    缺失的 `config`/`error`/`loop` 不再被写成 `null`。
  - `serial/session.rs` + `session_open.rs`：`setSignals` 与 open/close 同队列（脉冲进行中的调用排队）；
    `rtscts` 只拒绝显式 `rts`（脉冲照常执行）；`startLoop` 静态校验、静默替换旧任务；
    `stopLoop` 无循环时无事件；`set_status` 离开 `open` 时立即停循环；`Drop` 兜底关闭端口。
  - `serial/service_signals.rs`：服务层入口（未知路径 `notOpen`；`stopLoop` 空操作）。
  - `serial/transport_native.rs`：`TIOCMGET` → 改 `TIOCM_DTR`/`TIOCM_RTS` → `TIOCMSET`；
    fd 改为 `AtomicI32`（`-1` 收口，重复 `close` 是空操作，`Drop` 兜底），关闭后的写/信号返回 `notOpen`。
  - **连接关闭收口**：`SerialService::shutdown` 改为异步并关闭所有会话（TS `disposeAllAndWait`）——
    否则循环任务持有 `Arc<SerialSession>`，连接关闭后仍会继续写、端口 fd 也不释放。
- **通道**：`domains_serial.rs` 新增 `setSignals`/`startLoop`/`stopLoop`；`startLoop` 复用 `param_bytes`；
  `count` 缺省/非法区分（JSON `null` 与 `<1` 都判非法）；`inventory_services.rs` 补 3 条。
- **测试**：新增 20 例（`signals_tests` 3、`loop_runner_tests` 5、会话 S4 10、服务 2）+ 端到端 S4 段
  （未知路径：`setSignals`/`startLoop` 回 `SerialError + notOpen`，`stopLoop` 回 undefined）。
- **负向验证**（两项，均已恢复）：①去掉 250ms 进度节流 → 「100ms 内 0 条进度」失败（实测 10 条）；
  ②去掉「离开 open 自动停循环」→「关闭后立即不带 loop」失败。
  **第二项第一版测不出来**：关闭后下一 tick 的写入会因 `notOpen` 自行结束，短间隔下观察不到差别；
  已补「5s 间隔 + 关闭后立即断言」的用例专门钉住这条语义。
- **门禁**：workspace 全部测试目标通过（host-services 417、host-rpc 119 及全部 e2e）；
  clippy `-D warnings` 0；`pnpm check:escode-host`（边界 + 清单生成）通过；库存 **273 已实现 / 265 待办**。
