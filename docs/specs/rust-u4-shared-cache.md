# Rust 阶段 4：跨实例共享缓存（rust-unified-host.md 超越项 2）

父计划：`docs/specs/rust-unified-host.md` §阶段 4「跨实例共享缓存」。
前置：`docs/specs/rust-u2-provididers.md` 的 provider 配置与 `P1 #7` 的 FileStamp/ChangeGate。

## 1. 问题与目标

嵌入 Host 里每个 workspace 一个 runtime 实例；每个实例在 `runtime.rs` 里各自执行
`Registry::from_env()`，各自读取同一份 builtin/personal provider 配置、各自构建模型表、
catalog/options 快照与 HTTP 客户端池。8 个 workspace 就有 8 份。

目标：把「按配置路径确定、与 workspace 无关」的 provider/模型注册表做成进程内共享的不可变
快照 + 失效信号；每 workspace 的额外成本目标 ≤2.5 MiB（§5）。

本切片只做 **provider 配置 + 模型注册表**。skill/plugin 目录扫描按下节边界单独排期。

## 2. 范围与非目标

范围内：

- `escode-cli-model::registry::Registry`：新增进程级共享缓存（按 `(builtin, personal)` 路径键、
  `Weak` 持有），`from_env_shared()` 供 runtime 使用；`from_env()` 保留给独立进程/测试。
- `escode-cli-rust/src/runtime.rs`：嵌入与 CLI 都走 `from_env_shared()`。

非目标（本切片）：

- **child 链路**：`app-server --stdio` 是独立进程，共享缓存天然不跨进程。
- **skill/plugin 目录扫描**：已拆到 `docs/specs/rust-u4-directory-cache.md`（2026-10-28）：
  缓存键是「扫描根目录」，值是「目录指纹 + 解析后的技能条目」；指纹覆盖本次 walk 得到的
  `SKILL.md` 与同级 `_meta.json` 的 `(size, mtime_ms)`，scope/plugin 字段与
  `covered_by_escode` 过滤仍在每次发现时按当前 workspace 计算。
- **跨实例 catalog 推送**：共享快照变化后，其他实例仍按各自既有触发点（请求/账号更新）刷新
  catalog 帧；不做新的广播协议。

## 3. 所有权与数据流

```
runtime A ─┐
runtime B ─┼─ from_env_shared() ──▶ 进程级缓存 Map<(builtin,personal), Weak<Registry>>
runtime C ─┘                              │ 命中且存活 → Arc::clone
                                          │ 未命中 → Registry::new + refresh(None)
                                          ▼
                              唯一的 Snapshot（catalog/options/models/客户端池）
```

- 缓存只持有 `Weak`：registry 的生命周期仍由各实例的 `Arc` 决定，最后一个实例退出后条目可回收；
  查找时清理已失效条目，避免长期进程里按测试临时路径累积。
- 失效：沿用 `P1 #7` 的 `FileStamp` + `ChangeGate`（指纹未变跳过读取，兜底 60 s 全读）。
  共享后同一份文件只读一次，所有实例看到同一快照。
- 账号事实（`refresh(Some(account))`）在共享 registry 内是 Host 级状态：与本仓库 TS Host 的
  「Account Provider Registry 属于 Host」一致；同一 App 单账号登录。

## 4. 契约

- `from_env_shared()` 返回 `Result<Option<Arc<Registry>>>`；未配置任何路径时返回 `None`（与
  `from_env` 一致）。
- 同一 `(builtin, personal)` 路径对在同一进程内返回 `Arc::ptr_eq` 相同的实例。
- 不同路径对返回不同实例（测试/多环境隔离）。
- 共享实例上的 `refresh` 仍由串行入口保护；并发刷新不会互相覆盖快照。
- 退出语义不变：registry 不带后台任务、不注册 watcher；drop 即释放。

## 5. 验收

| 用例                         | 断言                                                                                     |
| ---------------------------- | ---------------------------------------------------------------------------------------- |
| 同路径两次 `from_env_shared` | `Arc::ptr_eq` 为真，且只有一次文件读取（Freeze 后 `refresh` 返回 false）                 |
| 不同路径两次                 | 非同一 `Arc`，各自快照独立                                                               |
| 全部 drop 后再取             | 新建实例（Weak 已失效，缓存不复活旧快照）                                                |
| 账号更新                     | 经 A 实例 `refresh(Some(account))` 后，B 实例 `received_account()` 可见同一账号          |
| 现有套件                     | `pnpm test:escode-cli-rust`、`pnpm test:escode-host` 不回退                              |
| 内存                         | app-perf workspaces：Host+Agent 斜率与共享前对比（本地与 CI 各记录，噪声范围内不硬断言） |

## 6. 已知差异与风险

| 项                   | 说明                                                                            |
| -------------------- | ------------------------------------------------------------------------------- |
| 账号状态改为 Host 级 | 多账号/多登录场景不支持；与 TS Host 的 Host 级账号注册表一致                    |
| 跨实例目录刷新时序   | 共享快照更新后，其他实例的 catalog 帧仍由各自触发点推送（与共享前同为最终一致） |
| 测试隔离             | 缓存键是路径对；临时目录唯一即可隔离。测试中显式不同路径不会被复用              |
| 与 Node 差分         | 单实例场景行为不变；多实例只有 Host 侧受益，CLI/stdio 不受影响                  |
