# Rust 阶段 2：embedded Host 存量会话导入（H3.5）

> 状态：立项 · 2026-10-22
> 父计划：`rust-unified-host.md` §阶段 2 / H3.5；前置：`rust-u1-runtime-lib.md` §3.4
> 现有语义：`docs/specs/rust-import-lifecycle.md`、`rust-session-loading.md`

## 1. 目标

App 级 Rust Host 默认以 **embedded** 运行 runtime。当前 `embed.rs` 明确“不做 TS 历史导入”，
导致切到 Rust Host 后 Node/TS 时代的会话历史不可见。本 spec 让 embedded 实例在**首次启动时**
按与 CLI `app-server` 完全相同的语义导入 TS 会话库，并保持幂等、可取消、失败不产生半截数据。

## 2. 所有权

- **runtime 实例是导入的唯一所有者**：锁、`Store`、SQLite 事务、导入账本、备份与取消都在
  `escode-cli-rust` 内，Host 不打开第二个 `Store`，也不在 Host 进程做预导入。
- Host 只通过 `RuntimeOptions` 提供**导入策略**；Main 不参与。
- 工作区 owner 锁（`workspace-<sha256(identity)>.lock`）在导入前获取，直到实例结束才释放。

## 3. 接口

`RuntimeOptions` 新增：

```rust
pub enum LegacyImport {
    /// 不导入（测试/显式关闭）。
    Disabled,
    /// 按 `legacy_paths::resolve(None, cwd, true)` 自动发现。
    Implicit,
    /// 显式来源；不存在即失败。
    Explicit(PathBuf),
}
```

- Host 的 `runtime_options_with` 默认 `LegacyImport::Implicit`；
- 需要显式来源时由 Main 经 `RpcListen.runtimeEnv` 提供 `ESCODE_SESSION_DB_PATH`/`ESCODE_SESSION_DB`，
  仍走 `Implicit` 的 `legacy_paths::resolve`（不新增协议字段）；
- child 链路后续对齐：`LegacyImport::Explicit(path)` → `--import-ts-db <path>`；
  `Implicit` 沿用 CLI 的 `config.is_none()` 语义。

## 4. 事件顺序

```text
resolve_workspace
  → Store::lock_workspace          (_owner 持有)
  → Store::open
  → set_model_usage_sink
  → startup/storageState checking  seq=1
  → import_history(policy, &cancel)
       → legacy_paths::resolve
       → Store::import_ts
            → data-dir import lock
            → 崩溃恢复未提交 attempt
            → 账本命中 → backfill todos，结束
            → 账本未命中 → 备份 + 单事务投影 + 写账本
  → Ok(false) → startup/storageState ready seq=2
  → build_engine → serve
  → drop(output) / finish(writer) / drain
```

约束：

1. `ready` 必须在导入返回之后发出；`Instance::mark_ready` 只认 `phase:"ready"`。
2. `Ok(true)` 表示取消：不发送 `ready`/`failed`，清理 writer 后正常结束实例。
3. `Err` 发送 `failed` + `errorCode:"sql_failed"`，实例结束；源库不被修改。
4. 重复启动由 `rust_legacy_import(source, workspace)` 账本幂等，不重复投影、不重复备份。
5. 导入期间 store actor 串行；工作区锁与 data-dir 导入锁排除并发写入。

## 5. 失败与取消

| 场景                            | 行为                                                             |
| ------------------------------- | ---------------------------------------------------------------- |
| 隐式来源不存在                  | `Ok(false)`，正常 `ready`                                        |
| 显式来源不存在                  | `Err` → `failed/sql_failed`，实例结束                            |
| TS schema 缺 `message.sequence` | `Err` → `failed/sql_failed`，无账本                              |
| 事务中失败                      | 单事务回滚；`ts-import-*` 清理；源库不变                         |
| 导入中 Host 关闭                | `Import` 取消；返回 `Ok(true)`；不产生半截数据；锁随实例结束释放 |
| 已导入                          | 账本命中，只做 `legacy_todos::backfill`，不重复备份              |
| COMMIT 与取消竞态               | 已提交结果为准；账本/备份保留；调用方按 `Ok(true)` 退出          |

## 6. 验收

- `embed_memory_link.rs`：
  - 首次启动导入 TS 会话 → `session/list`/`session/read` 可见；
  - 第二次启动账本仍为 1 行、备份不新增；
  - 取消导入 → 无 `ready`、无账本、无残留 attempt，随后可重新启动；
  - 显式来源不存在 → `failed/sql_failed`，源库哈希不变。
- `host-runtime/tests/instances.rs`：`Instances::get` 触发导入、懒重启不重复导入。
- `apps/escode-host/tests/host_entry.rs`：真实 Host `WindowAttach → ready → session/read`。
- `control_tests.rs`：`runtime_options_with` 默认 `Implicit`，显式 env 映射正确。
- 既有 CLI 迁移套件（scale/lifecycle/shapes）保持全绿，作为 oracle。
