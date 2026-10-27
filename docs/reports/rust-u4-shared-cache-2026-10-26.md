# Rust 阶段 4：跨实例共享缓存（2026-10-26）

父计划：`docs/specs/rust-unified-host.md` §阶段 4 第 2 项。契约：`docs/specs/rust-u4-shared-cache.md`。

## 1. 变更

- `escode-cli-model::registry::Registry` 新增进程级共享缓存：
  - `from_env_shared()`：按实例 ambient env 的 `(ESCODE_BUILTIN_PROVIDER_CONFIG_FILE,
ESCODE_PERSONAL_PROVIDER_CONFIG_FILE)` 路径对查找；`Registry::shared(builtin, personal)`
    是测试/嵌入接缝。
  - 缓存持 `Weak`，创建与登记在同一把同步锁内完成（并发首取不会产生两个快照）；查找时清理
    已回收条目。
  - 失效沿用 `FileStamp` + `ChangeGate`（P1 #7）：同一份文件只真读一次，`refresh` 仍由 serial
    段保护。
- `escode-cli-rust/src/runtime.rs`：嵌入与 CLI 都改用 `from_env_shared()`。child/stdio 进程
  不在同一进程，天然不共享（符合契约）。

## 2. 验收

### 2.1 单元测试（`crates/model/src/registry_tests.rs`，7 passed）

- 同路径对两次 `shared` → `Arc::ptr_eq`；不同路径对 → 不同实例。
- 最后一个持有者 drop 后，缓存条目被清理，下一次取到的是新实例（不复活旧快照）。
- 共享实例上的账号更新对另一个持有者可见（同一 Host 账号事实）。

### 2.2 每 workspace 内存（Linux Xvfb，N=1..8 八点回归，Host+Agent 子集斜率）

同一台机器、同一 E2E 用例，对照组是共享前的一个运行：

| runtime               |       共享前 |     共享后（2 次） | 变化               |
| --------------------- | -----------: | -----------------: | ------------------ |
| Node（未改动，对照）  | 10.44 MiB/ws |        9.12 MiB/ws | −12.6%（机器波动） |
| Rust（共享 registry） | 11.49 MiB/ws | 7.39 / 5.87 MiB/ws | −36% / −49%        |

- 共享前 Rust 比 Node 高约 10%；共享后 Rust 比 Node 低约 19–36%。方向与「消除每实例模型表/
  目录快照/客户端池」一致。
- 单次运行的绝对值受 renderer GC 与机器负载影响（本报告里的 Node 对照也漂了 12.6%），因此
  不把绝对阈值写成门禁；CI 的 `app perf` 只输出趋势表。

### 2.3 未达到/未覆盖

- §5「每多一个 workspace ≤2.5 MiB」仍未达到：共享后 Rust 斜率 5.9–7.4 MiB/ws，剩余成本主要
  是每个 workspace 的会话运行时状态（会话行、prompt 快照、SQLite 缓存），不是 provider 配置。
- **skill/plugin 目录共享未做**：项目级技能按 cwd 作用域，缓存键必须包含目录集合与逐目录指纹，
  错误共享会把 A 项目的技能泄漏给 B。契约里已登记为缺口，等目录失效方案单独评审。

## 3. 风险与差异

- 账号状态改为 Host 级：与 TS Host 的 Host 级账号注册表一致；多账号场景不支持。
- 跨实例 catalog 更新仍靠各实例既有触发点，不新增广播协议（与共享前同为最终一致）。
- 测试隔离依赖路径对唯一；`Registry::shared` 的隔离测试已覆盖。

## 4. 验证

- `cargo test -p escode-cli-model registry_tests`：7 passed。
- `pnpm check:escode-cli-rust`、`pnpm check:escode-host`：exit 0（boundary + fmt + clippy -D warnings）。
- `pnpm test:escode-host`、`pnpm test:escode-cli-rust`（CI 同口径 serial + 60s 预算）：见 CI 结果。
- `pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed`：通过。
