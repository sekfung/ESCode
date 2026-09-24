# Rust runtime 发布与回退（WP10 前置）

2026-09-24。本文定义 runtime 切换面、回退步骤与验收；**当前默认仍是 Node runtime**，只有下列门槛全部通过才切换。

## 切换面

App 通过 Host 启动参数选择 Agent runtime（`packages/services/src/zcode-agent/zcodeAgentProcessManager.ts`）：

| 变量                           | 作用                                                                                                                               |
| ------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------- |
| `ZCODE_AGENT_SERVER_COMMAND`   | Agent 可执行文件；指向 Rust 二进制即启用 Rust runtime                                                                              |
| `ZCODE_AGENT_SERVER_RUNTIME`   | 仅接受 `zcode-cli-rust`；设它才会附加 `--cwd <workspacePath>`、`storagePreparationMode: "process"`、`supportsStorageStartup: true` |
| `ZCODE_AGENT_SERVER_ARGS_JSON` | 附加参数；Rust 模式下不允许自带 `--cwd`（Host 注入）                                                                               |

未设置这些变量时，Host 启动随包分发的 Node CLI——这就是回退路径。

## 回退步骤（可执行）

1. 移除/清空 `ZCODE_AGENT_SERVER_RUNTIME`（以及只想用 Node 时的 `ZCODE_AGENT_SERVER_COMMAND`）；
2. 重启 App（或重建窗口的 Local Host）；
3. 确认 Agent 进程是 Node CLI（`zcode-agent` 日志中的 runtime 事件）。

回退**不需要**数据迁移：Rust runtime 只读 TS 存储（见下），Node 继续使用原有 `ts.sqlite`。

## 数据边界与已知限制

- Rust 导入 TS 数据时只读源库并以备份方式复制（`crates/state/src/legacy_storage.rs`），源库与 TS 附件目录逐字节不变，因此回退无损。
- Rust 自己的会话写在 `<dataDir>/rust-sessions.sqlite`。**回退到 Node 后这些会话在 Node 侧不可见**（Node 不读该库）；数据仍在磁盘上，重新启用 Rust 即可再次打开。若产品要求回退后仍能看到 Rust 期会话，需要新增导出/反向迁移，当前未实现，属已知限制。
- workspace 独占锁（`workspace-<hash>.lock`）在进程退出后释放；回退或重启后另一 runtime 可立即接管同一 workspace。

## 发布门槛（全部通过才切换默认 runtime）

| 维度     | 验收方式                                                                   | 现状                                                                                        |
| -------- | -------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------- |
| 功能对齐 | App 集成套件 + 各能力差分语料（权限、Bash、代理、prompt 等）               | Windows 上 196 用例 194 通过、2 跳过（理由见 rust-windows-parity.md）；**macOS/Linux 未跑** |
| 性能     | 首段延迟、会话加载、空闲驻留、大历史查询（见 rust-runtime-performance.md） | 仅一项实测（首个模型请求 1830ms → 35ms）；完整口径未验收                                    |
| 数据迁移 | TS 导入用例 + 源库字节不变 + 大库实测                                      | 用例通过；**大库/真实用户数据未实测**                                                       |
| 跨平台   | Windows 已完成；macOS/Linux 需各自构建并跑同一套件                         | **仅 Windows**（且为 GNU 目标，MSVC 未跑）                                                  |
| 发布回退 | 本文两条自动化用例 + 回退步骤演练                                          | 用例通过；**未做真实发布演练**                                                              |

补充硬性要求：发布验收必须用 MSVC 目标构建（本机当前缺 MSVC，只能用 GNU 目标得出源码级结论）。

## 自动化验收（已实现）

`packages/services/tests/zcode-cli-rust-rollback.test.ts`：

1. `Rust import leaves TS storage byte-identical and frees the workspace for rollback`
   - 导入后 `ts.sqlite` 的 sha256 与导入前一致；TS store 仍可读到原会话；
   - Rust 进程退出后可再次启动并读到会话（owner lock 已释放）；
   - 第二轮运行后源库仍逐字节不变。
2. `Rust storage stays readable after the process exits`
   - `rust-sessions.sqlite` 在进程退出后可直接读取，结构化事实（mode 等）确实落盘。
