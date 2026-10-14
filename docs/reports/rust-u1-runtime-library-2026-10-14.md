# Rust 阶段 1（runtime 库化）验收报告

> 日期：2026-10-14
> 子 spec：`docs/specs/rust-u1-runtime-lib.md`；父计划：`docs/specs/rust-unified-host.md` 阶段 1
> 参考实现：ZCode-rs `apps/zcode-cli-rust/src/embed.rs`（Embedded 形态）、`rust/crates/kit/src/fs/stamp.rs`（FileStamp/ChangeGate 语义）

## 1. 结论

阶段 1 六项交付物全部完成并通过验收；runtime 现在可作为库在同一进程内 spawn 多个互不串扰的实例，具备双传输（stdio / 内存）、显式环境、实例任务作用域与进程内 owner 锁登记表。P1 结构对齐完成 #5 与 #7，#8 完成三项中的两项，#12 经核查与对齐契约冲突而挂起（理由见下）。

## 2. 验收矩阵（父计划阶段 1 验收行）

| 验收项 | 结果 | 证据 |
| --- | --- | --- |
| cwd/env 与 runtime 不一致时用 `RuntimeOptions` 的值 | 通过 | `tests/embed_memory_link.rs::runtime_options_env_and_cwd_drive_workspace_resolution`——相对 `ESCODE_CLI_RUST_DATA_DIR`（实例 env 快照）相对实例 cwd 解析，`startup/storageState.databaseId` 等于实例路径的 sha256，且存储文件落在实例目录 |
| 同进程两 workspace 并发无串扰 | 通过 | 同文件 `two_instances_run_concurrently_without_crosstalk`——A 建会话后 A 的 `session/list` 可见、B 为空；A 收口后 B 继续服务；usage sink 为实例槽（`env.rs` 实例覆盖 + 进程回退） |
| 关闭后再 spawn 正常 | 通过 | 同文件 `duplicate_ownership_is_rejected_and_respawn_after_close_succeeds`——同 workspace 第二个实例以 `WorkspaceOwned` 拒绝（进程内登记表 + 文件锁双保险），首个 EOF 收口后再 spawn 成功 |
| 环境读取检查为 0 | 通过 | `scripts/check-escode-cli-rust-boundaries.mjs` 新规则：crates/ 非测试代码 0 处 `std::env::{var,var_os,vars,vars_os,set_var,remove_var,current_dir,current_exe,args}`；唯一豁免 `core-api/src/env.rs`；`temp_dir` 为进程级语义保留并登记 |
| 双传输差分全绿 | 通过 | `tests/dual_transport.rs` 三用例：同一字节流（含无效 JSON / 空白行 / 无换行尾行）两条链路产出同一 Input 序列；输出帧字节等价；EOF 与关闭语义等价。实现上两条链路共用 `stdio::start_io` 同一段读写代码，等价性由同一实现保证而非两套对拍 |
| 进程内控制延迟基线 | 记录 | p50 **0.250 ms** / p95 **0.334 ms**（n=50，Linux x86_64，debug，MemoryLink NDJSON 路径）。与 ZCode-rs 进程内 duplex+NDJSON（0.2–0.3 ms）同量级；阶段 4 免序列化直传目标 ≤0.1 ms |
| 实例级收口断言 | 通过 | 各用例收口后 `tasks.running()` 为空；`Embedded` 根任务结束时 `abort_all()` 中止残余（对标 ZCode-rs scope 语义） |

其它全量门禁（本轮同批修改后复跑）：

| 门禁 | 结果 |
| --- | --- |
| `cargo fmt --all --check` | 通过 |
| `cargo clippy --locked --workspace --all-targets`（-D warnings） | 0 error 0 warning |
| `cargo test --workspace` | 415 通过 / 0 失败（含新增嵌入 5、双传输 3、Spawner 5、FileStamp 5、registry 4、waiters 2） |
| `node scripts/check-escode-cli-rust-boundaries.mjs` | OK（新增环境读取规则） |
| `pnpm test:escode-cli-rust`（差分 + 生成器门禁） | 见 §5 复跑记录 |
| `pnpm typecheck` / `pnpm lint` / `pnpm architecture:check --changed` | 通过 / 0 error / 0 违规 |

## 3. 架构与所有权

```
RuntimeOptions ──► EnvSnapshot ──► INSTANCE task-local ──► env 访问器（crates/ 唯一进程事实来源）
                             └──► Spawner（唯一 spawn 入口：登记、panic 上报、env 传播、drain）

embed::spawn ─► 实例根未来（INSTANCE.scope）
   ├─ resolve_workspace（RuntimeOptions 优先，无进程 env 回退依赖）
   ├─ Store::lock_workspace → WorkspaceOwnerGuard（进程内登记表 token + 文件 flock）
   ├─ Store::open（usage sink 写实例槽）
   ├─ build_engine（RuntimePorts.spawner 注入 Engine / WorkspaceTools）
   ├─ MemoryLink ► engine.serve（与 stdio 同一 start_io 代码路径）
   └─ 收口：serve 返回 → drain(5s) 残余清单告警 → abort_all → 任务 JoinHandle 返回
```

状态所有者：runtime 仍是会话事实的唯一所有者（Engine actor 未变）；`Spawner` 拥有实例任务账目；`InstanceContext` 拥有环境快照、实例级 usage sink 与媒体缓存根；owner 锁的进程内登记表在 `crates/state`，文件锁语义不变（exit 75 保留）。

## 4. 与参考实现（ZCode-rs）的对照

| 维度 | ZCode-rs | 本方（u1） | 备注 |
| --- | --- | --- | --- |
| 嵌入入口 | `embed::spawn(AppServerOptions) -> Embedded { io, task, cancel, tasks }`（`tokio::io::duplex`） | `embed::spawn(RuntimeOptions) -> Embedded { io, task, cancel, tasks }`（mpsc 字节通道 + 按行缓冲） | 形态同构；本方 `RuntimeOptions` 显式携带 home/env/argv/cwd/identity/data_dir/config |
| 作用域 | `zcode_async::Spawner`，实例任务结束时中止残余 | `core_api::scope::Spawner`，语义一致（`running()`/`drain()`/`abort_all()`）+ panic sink 上报 | 本方 panic 重抛保留 `JoinError::is_panic` 语义 |
| 环境 | 组合根传入（其 spec §3） | 进程级快照 + 实例快照，边界脚本强制 0 处直读 | 本方多一条机器门禁 |
| file stamp | `kit::fs::{FileStamp, ChangeGate}`（len/mtime/inode/ctime + 兜底全读） | `host::file_stamp` 同语义，兜底 60s，接入 registry 每秒刷新 | 五类稳定性测试（出现/改写/原子替换/原地写/删除）+ 闸门三态测试 |
| owner 锁 | 已删工作区锁（靠 Host lease，其 spec 自认风险） | **保留** owner 锁 + exit 75，另加进程内登记表与实例 token | 保留清单项，不随对齐抄丢 |

## 5. 差分与回归

- `cargo test --workspace`（本批全部改动后）：**433 通过 / 0 失败**（含新增的嵌入 5、双传输 3、Spawner 5、FileStamp 5、registry 4、waiters 2、对账 4、secret 2）。
- 差分套件（`pnpm test:escode-cli-rust`，407 用例 + 27 个生成器 `--check` 门禁）：
  - 第一轮（迁移批次构建，2026-10-14）：393 通过 / 2 失败 / 12 跳过。
  - 失败一 `escode-cli-rust-file-log.test.ts` 单独复跑**通过**——该轮失败与并行构建擦写同一个 `target/debug/escode-cli-rust`（我在套件运行期间执行了 `cargo clippy`/`cargo test`）有关；教训：套件运行期间禁止并发 cargo 构建。
  - 失败二 `escode-cli-rust-legacy-session.test.ts`（"legacy session methods behave the same on Node and Rust"，报 `ESCode agent stdio transport is closed`）：**本机既存问题，非本批引入**。判据（bisect）：把 `apps/escode-cli-rust` 恢复到阶段 0 提交 `1d94fb38` 重建基线二进制后，同一用例以同一错误签名稳定失败（默认 env 与 CI env 各一次）；本批二进制同样失败。该用例的 Rust 侧在 slow 发送→settle 之后、`session/close` 前后进程提前结束，宿主应答因传输关闭抛错，具体原因待单独排查（阶段 2 差分维护项）。
  - 最终复跑（本批代码，无并发构建，2026-10-14）：**407 例 / 392 通过 / 3 失败 / 12 跳过**。三个失败的分类（逐个隔离复跑）：`escode-cli-rust-file-log.test.ts` 与 `escode-cli-rust-amend-workflow.test.ts` 在隔离运行下**通过**（满载串行下超时/竞争，属套件负载浮变）；`escode-cli-rust-legacy-session.test.ts` 隔离运行同样失败，且阶段 0 基线二进制复现同一签名——**本机既存问题，非本批引入**。
- 与 TS oracle 的协议面未变更：双传输两端都产出与 stdio 相同的 NDJSON 帧；`session/list`、`createSession`、`v4/commands/query` 等应答形状未改。

## 6. 遗留与移交（u2 输入）

0. **已知失败**：`escode-cli-rust-legacy-session.test.ts` 在本机对阶段 0 与本批二进制同样失败（见 §5 判据），列为本机环境既存问题；阶段 2 差分维护项，需在本机以外（CI 三平台）确认是否复现。
1. `crates/host` 更名/文档注明（与新 Host 概念重名）。
2. net 出口收拢（P1 #6）、plugins crate 拆分（#9）、`v4/command` schema 校验（#10）、bash registry 类型化（#11）、typed 方法分派与瞬态 `Submission`（#8 剩余两项）。
3. 遥测 ChunkWindow（#12）挂起：协议 strict schema 与 TS oracle 均无窗口语义，实施将破坏 §4.1；条件见父计划阶段 1 注记。
4. `net_config` 文件层、file_log、host/browser broker 仍为进程级（登记为已批准差异）；SQLite actor 与 Link 线程以句柄收口，未纳入 Spawner 账目（理由见子 spec §3.1）。
