# Rust 阶段 1：runtime 库化（rust-unified-host.md 子 spec）

> 状态：草案 · 2026-10-14
> 父计划：`rust-unified-host.md` §阶段 1；质量前置：同文件「实现质量对齐工作包」P1 #5
> 子 spec 约定：本文覆盖库化本体 + P1 #5 TaskScope。P1 #6–#12（net 出口收敛、FileStamp、waiter 合并、plugins crate、schema 校验、bash registry、ChunkWindow）仍在阶段 1 范围内并行推进，各有独立验收，不阻塞本文交付。

## 1. 目标

把 runtime 从「只能整进程作为 `app-server --stdio` 子进程运行」改造为「可作为库在同一进程内 spawn 多个互不串扰的运行时实例」，为阶段 2 的 Rust Host（`apps/escode-host`）提供嵌入入口。

不改变的行为：协议面（V4 帧）、存储格式、owner 锁 + exit 75、stdio 二进制入口、headless `-p` 入口、wiki 子命令。

非目标（后续阶段）：Host 进程骨架与分域服务（阶段 2，子 spec `rust-u2-*.md` 立项时确认 window→App 作用域边界并处理 `crates/host` 与新 Host 的重名问题）；`MemoryLink` 免序列化直连（阶段 4）；`CoreRuntime` 第二所有权清理（P1 #8）。

## 2. 交付物

1. **P1 #5 TaskScope**（先行，多实例与收口的前置）。
2. **显式环境**：crates/ 非测试代码 0 处 `std::env::var*` / `current_dir` / `current_exe` / `args` / `set_var` / `remove_var`；env 快照由组合根传入；新增边界脚本规则。
3. **`Link` trait + 双传输**：`StdioLink`（现有 stdin/stdout）+ `MemoryLink`（内存双工 + NDJSON 保等价；接口为阶段 4 免序列化预留）。
4. **`embed::spawn`**：`RuntimeOptions { home, env, argv, cwd, workspace_identity, data_dir, config } -> Embedded { io, task, cancel, tasks }` 公共入口。实现补充：`config` 是父计划清单外的必要项——`createSession` 需要模型选择，嵌入方（Host）必须能注入静态 provider 配置（同 CLI `--config`）。
5. **多实例**：同进程 2+ workspace 并发；owner 锁进程内登记表 + 实例 token 语义；同进程关闭后再 spawn 正确。
6. **双传输一致性子测试** + **进程内控制延迟基线**。

## 3. 设计

### 3.1 实例上下文与 TaskScope（core-api）

新模块 `crates/core-api/src/env.rs`（快照与实例上下文）与 `scope.rs`（Spawner）。core-api 是所有 crate 的公共依赖（core/model/tools/state/host/wiki/app-server 均依赖它），且已有 task-local 先例（`ModelCallScope`）与 panic 先例（`catch_panic`），env/scope 放这里不引入新依赖方向。

```rust
// env.rs
pub struct EnvSnapshot {
    vars: HashMap<OsString, OsString>,   // Windows 追加小写键索引，复刻 std::env::var 的大小写不敏感语义
    cwd: PathBuf,
    exe: PathBuf,
    argv: Vec<String>,
}
impl EnvSnapshot {
    pub fn capture_process() -> Self;                 // 组合根（main.rs）用：真实进程 env/cwd/exe/args
    pub fn from_parts(vars, cwd, argv, exe) -> Self;  // embed::spawn 用：RuntimeOptions 值
}
pub struct InstanceContext {
    pub env: Arc<EnvSnapshot>,
    pub spawner: Weak<Spawner>,
    pub usage_sink: OnceLock<ModelUsageSink>,      // 实例级覆盖（多实例不串扰；进程级回退保留）
    pub media_cache_root: OnceLock<PathBuf>,
}
tokio::task_local! { static INSTANCE: Arc<InstanceContext>; }
```

访问器（`escode_cli_core_api::env::{var, var_os, vars, vars_os, current_dir, current_exe, args, temp_dir}`），解析顺序固定：

```
实例 task-local（embed 实例树内）→ 进程全局快照（binary main 安装）→ 真实进程 env（两者皆未安装时兜底，保持既有单测行为）
```

兜底是兼容路径而非逃生门：纪律由边界脚本强制（§3.5）。`temp_dir` 保持进程级语义（`TMPDIR` 不随实例隔离，登记为已批准差异）。

```rust
// scope.rs
pub struct Spawner { env, sink, tasks: Mutex<BTreeMap<u64, String>>, next: AtomicU64 }
impl Spawner {
    pub fn new(env: Option<Arc<EnvSnapshot>>) -> Arc<Self>;
    pub fn with_panic_sink(self: Arc<Self>, sink: PanicSink) -> Arc<Self>;
    pub fn current() -> Arc<Self>;   // 实例树内 → 实例 Spawner；否则以当前快照新建临时 scope
    pub fn spawn<F>(&self, name: &'static str, fut: F) -> JoinHandle<F::Output>;
    pub fn spawn_blocking<F, T>(&self, name: &'static str, f: F) -> JoinHandle<T>;
    pub fn running(&self) -> Vec<String>;                     // 残余任务名清单
    pub async fn drain(&self, grace: Duration) -> Vec<String>;// 宽限等待，返回仍残余的名字
}
```

语义：

- **唯一 spawn 入口**：实例树内所有 `tokio::spawn` / `tokio::task::spawn_blocking` 改经 Spawner。不进 scope 的例外及理由：Link 传输线程（reader/writer，随 Streams 句柄收口）、file_log 进程级单例线程、`Store::open` 的 SQLite actor（随最后一个 `Store` 克隆 drop 结束）。
- **panic 上报**：spawn 内 `catch_unwind` → 记录 + sink 回调（组合根接 `file_log::error`）→ `resume_unwind` 重抛，await 方仍按 `JoinError::is_panic()` 观测，不改变既有错误语义。
- **env 传播**：spawn 重新建立 `INSTANCE.scope`，子任务与 `spawn_blocking`（`sync_scope`）都看到实例快照；embed 模式下深 crate 的 `spawn_blocking`（如 model 的 HTTP client 构建）因此拿到正确 env。
- **关闭宽限**：`drain(grace)` 由 embed 收口路径调用；条目在 future 结束（含 panic、abort）时由 guard 注销。

迁移点（u1 收口，2026-10-18 M1 复核）：core 16 处、tools 20 处（含本轮补齐的 14 处裸 `spawn_blocking`）、model `spawn_blocking` 4 处、state owner 锁与 usage recorder、wiki 解析、app-server 停滞看门狗。收口后非测试代码只剩两处直接调用：`core-api/src/scope.rs`（Spawner 实现本身）与 `state/src/storage.rs` 的 `Store::open` SQLite actor（§3.1 例外）。`Spawner::current()` 供无构造管道的深 crate（model/wiki/state）使用；Engine/WorkspaceTools 经 `RuntimePorts.spawner` / `with_spawner` 显式持有同一实例（测试隔离也依赖显式注入）。

### 3.2 实例级全局状态归属

| 状态                                                     | owner（u1 起）        | 说明                                                                                             |
| -------------------------------------------------------- | --------------------- | ------------------------------------------------------------------------------------------------ |
| env/cwd/argv/exe                                         | `InstanceContext.env` | 实例树内一律走访问器                                                                             |
| spawn 账目与 panic                                       | `Spawner`             | 每实例一个                                                                                       |
| usage sink                                               | 实例覆盖 + 进程回退   | `set_model_usage_sink` 在实例树内调用时写实例槽                                                  |
| media cache root                                         | 实例覆盖 + 进程回退   | 同上                                                                                             |
| network 文件层（`net_config::install`）                  | 进程级（u1 维持）     | 多实例同进程且 workspace 网络配置不同时会互相覆盖；u2 随 Host 装配迁入实例上下文，登记为已知债务 |
| file_log、Windows 代码页、locale 头、host/browser broker | 进程级                | 派生自进程环境/进程单例，与实例身份无关，登记为已批准差异                                        |

### 3.3 Link 双传输（app-server）

```rust
pub struct Streams {
    pub input: mpsc::Receiver<Input>,
    pub output: Output,
    pub writer: std::thread::JoinHandle<()>,   // 既有 finish() 排空语义不变
}
pub trait Link: Send + Sync + 'static {
    fn start(&self, cancel: CancellationToken, input_closed: CancellationToken) -> Streams;
}
pub struct StdioLink;                                     // 现 stdin/stdout 行为逐字节不变
pub struct MemoryLink;  impl MemoryLink { pub fn channel(buf: usize) -> (Self, EmbeddedIo); }
pub struct EmbeddedIo { /* inbound 行发送 + outbound 行接收 */ }
```

- `stdio_input.rs` 的读取/写出线程泛型化：reader `R: BufRead + Send + 'static`，writer `W: Write + Send + 'static`。行切分、`MAX_REQUEST_BYTES`、`dispatch`、停滞看门狗、EOF → `input_closed` 全部共享——等价性由「同一段代码」保证，而非两套实现对拍。
- `MemoryLink`：inbound 经 `mpsc<Vec<u8>>` 的 `ChannelRead`（reader 线程 `blocking_recv`，sender 全部 drop = EOF）；outbound 经 `blocking_send` 进有界通道（writer 线程阻塞 = 背压，停滞看门狗语义与 stdio 一致）。NDJSON 帧面与 stdio 相同，接口形状为阶段 4 换成类型化消息直传预留。
- `EmbeddedIo::drop` = 关闭 inbound → 实例走 EOF 收口；`io.recv()` 返回 `None` = 实例 output 已排空关闭。

### 3.4 embed 入口（src/embed.rs，lib 目标）

```rust
pub struct RuntimeOptions {
    pub home: PathBuf,
    pub env: BTreeMap<String, String>,   // 实例可见的完整环境（缺 HOME/USERPROFILE 时以 home 补）
    pub argv: Vec<String>,
    pub cwd: PathBuf,                    // Host 提交的 workspace 路径（不 realpath）
    pub workspace_identity: Option<String>,
    pub data_dir: Option<PathBuf>,       // 缺省按 env 快照解析（与 CLI 相同规则）
}
pub struct Embedded {
    pub io: EmbeddedIo,
    pub task: JoinHandle<anyhow::Result<()>>,
    pub cancel: CancellationToken,
    pub tasks: Arc<Spawner>,             // running() 残余清单
}
pub fn spawn(options: RuntimeOptions) -> Embedded;
```

实例根事件顺序（嵌入模式）：

```
spawn() ──► 构造 EnvSnapshot + Spawner ──► INSTANCE.scope(根未来)
   根未来：resolve_workspace(用 RuntimeOptions) → lock_workspace(进程内登记表 → 文件锁)
        → Store::open → （无 TS 导入：embed 假定存储已就绪/由调用方控制）
        → build_engine(实例 Spawner 经 RuntimePorts 注入 Engine 与 WorkspaceTools)
        → engine.serve(MemoryLink streams, cancel)
   关闭：调用方 drop EmbeddedIo / cancel.cancel() → serve 收口（同 stdio 语义）
        → drop store → tasks.drain(grace) → task 返回 Result
```

- 不监听信号、不 `process::exit`：信号与退出码是进程概念，归调用方（binary main / 未来的 Host）。
- `WorkspaceOwned` 以 `anyhow::Error` 类型返回（`downcast_ref::<WorkspaceOwned>()`），不映射退出码。
- `wiki` / `-p` 子命令不在 embed 面内；`src/runtime.rs` 移入 lib 目标与 embed 共用装配（resolve_workspace / build_engine / import_history）。

### 3.5 边界脚本规则

`scripts/check-escode-cli-rust-boundaries.mjs` 新增：crates/ 全部 `.rs` 文件，剔除 `tests/` 目录、`*_tests.rs` 测试模块与 `#[cfg(test)]` 段后，禁止 `std::env::{var, var_os, vars, vars_os, set_var, remove_var, current_dir, current_exe, args}` 与 `env::current_dir` 等裸调用。唯一豁免：`core-api/src/env.rs`（被批准的进程事实采集 shim，内含注释说明）。src/（binary 组合根）不在本规则范围。

同一脚本新增 spawn 纪律规则：上述非测试代码禁止裸 `tokio::spawn` / `tokio::task::spawn_blocking`，唯一 spawn 入口是 `Spawner`。例外按**文件 + 理由**显式登记，不按行号：`core-api/src/scope.rs`（Spawner 实现本身）与 `state/src/storage.rs`（`Store::open` 的 SQLite actor，§3.1 例外）。Link 传输线程、file_log 单例线程使用 `std::thread`，不落入该规则。

### 3.6 owner 锁进程内登记表（crates/state）

```
lock_workspace(dir, identity)：
  1. 进程内登记表 Mutex<HashMap<lock_path, token>> 命中 → Err(WorkspaceOwned)
  2. 文件 try_lock（跨进程语义不变；同进程第二把 flock 也因 OFD 冲突失败，双保险）
  3. 成功 → 登记表写入本实例 token → 返回 WorkspaceOwnerGuard
WorkspaceOwnerGuard::drop：先注销登记表条目，再释放/关闭文件锁（关进程也安全）
```

`lock_workspace` 返回类型 `std::fs::File` → `WorkspaceOwnerGuard`（调用点：main.rs、headless、embed）。token 是进程内唯一 uuid，随 Guard 写入锁文件供诊断（锁文件内容今日无任何消费方，写入不破坏协议）。同进程「关闭后再 spawn」= 前一实例 Guard drop 后登记表清空，重新 spawn 走全流程。

## 4. 验收（对应父计划阶段 1 验收行）

| 验收项                                              | 证据                                                                                                                |
| --------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| cwd/env 与 runtime 不一致时用 `RuntimeOptions` 的值 | 测试：进程 cwd/env 与实例不同，实例内 workspace 解析、identity、上下文端口读到 RuntimeOptions 值                    |
| 同进程两 workspace 并发无串扰                       | 测试：两实例不同 data_dir 并发 createSession/sendText（fake model），会话事实互不可见；usage sink 互不串扰          |
| 关闭后再 spawn 正确                                 | 测试：spawn → EOF 收口 → 同 options 再 spawn（锁已释放）成功；同 options 双 spawn 第二个返回 `WorkspaceOwned`       |
| 环境读取检查为 0                                    | 边界脚本规则通过（`pnpm check:escode-cli-rust`）                                                                    |
| 双传输差分全绿                                      | 新增一致性测试：同场景经 MemoryLink 与 StdioLink（`std::io::pipe`）产出相同帧序列/顺序/关闭语义；既有差分套件不回退 |
| 进程内控制延迟基线                                  | `examples/embed_control_latency.rs` 实测 capabilities 往返 p50/p95，数字回写本文件 §6                               |
| 实例级收口断言                                      | 测试：serve 返回后 `tasks.running()` 为空                                                                           |

## 5. 测试落点

- `apps/escode-cli-rust/tests/embed_memory_link.rs`：嵌入实例生命周期（spawn/并发/重启/双 spawn 冲突/收口 running() 空/cwd-env 隔离）。
- `apps/escode-cli-rust/tests/dual_transport.rs`：MemoryLink vs StdioLink 帧等价（复用 `tests/support` 桩）。
- `crates/core-api/src/scope.rs` 内联单测：panic 上报、drain 宽限、env 传播。
- `crates/state` 内联单测：登记表冲突与 Drop 注销。
- 既有套件（`tests/*.rs`、差分脚本、`pnpm test:escode-cli-rust`）不回退。

## 6. 基线记录

进程内控制 RPC 往返（`tests/embed_memory_link.rs::in_process_control_round_trip_latency_baseline`，
2026-10-14，Linux x86_64，debug 构建，MemoryLink NDJSON 路径）：

| 指标 |     实测 | 对标（ZCode-rs 进程内 duplex+NDJSON） |             阶段 4 目标 |
| ---- | -------: | ------------------------------------: | ----------------------: |
| p50  | 0.250 ms |                            0.2–0.3 ms | ≤0.1 ms（免序列化直传） |
| p95  | 0.334 ms |                                     — |                       — |

与对标仓库同量级，符合阶段 1 只做链路抽象的定位；阶段 4 换类型化直传后按上表收紧。

## 7. 移交与债务（u2 输入）

- `crates/host` 更名或文档注明（父计划 §阶段 2 已列）。
- `net_config` 文件层、host/browser broker、file_log 的实例化迁移。
- SQLite actor / Link 线程纳入统一账目（当前以句柄收口，理由见 §3.1）。
- P1 #6–#12 未完成项随各自验收推进。
