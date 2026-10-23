# Rust 统一 Host：阶段 2/阶段 3 收口进度（2026-10-23）

父计划：`docs/specs/rust-unified-host.md` §阶段 2 / §阶段 3。本文记录本轮实际执行的验收与剩余项，
不把“已实现”写成“已验收”。

## 1. 阶段 2：服务域与验收

### 1.1 契约清单

`pnpm check:escode-host`（exit 0）：

- 43 channel / 495 成员，436 Implemented；
- 59 Pending 全部是已登记非缺口：
  - `escode-agent` 54 个别名（`rust-u2-host-scope-boundary.md` §8）；
  - `cua-pip-session` 3（无 Renderer 调用，§2.2）；
  - `provider-provisioning-target.apply` 1（零调用，§2.2）；
  - `escode-session.readSessionEvents` 1（运行时无 `session/events`，§8.4）。
- 生成器 `--check`、边界、fmt、clippy `-D warnings` 同批通过。

### 1.2 域验收补充

- automation：新增 `packages/desktop/test/rustHostAutomationParity.test.ts`。同一 tasks-index 库上
  TS `AutomationService` 与真实 `@escode/client` 连 Rust Host 交替创建/暂停/改名/删除，
  双方 `list` 逐字段全等（1 passed）。
- providers：`rust-u2-providers.md` 的 P6（`credential` 3/3、`client-config` 2/2、`client-scenes` 1/1）
  由契约清单确认已交付，文档状态已更新。
- 其余域状态以各子 spec 的进度表为准；本轮不重复测试已由 M1–M3 覆盖的域。

### 1.3 阶段 2 验收项

| 验收项                         | 证据                                                                 | 结果                                               |
| ------------------------------ | -------------------------------------------------------------------- | -------------------------------------------------- |
| 契约清单 `--check`             | `pnpm check:escode-host`                                             | 通过                                               |
| Host cargo 全量测试 + interop  | `pnpm test:escode-host`                                              | 通过（含新增冻结夹具用例）                         |
| 多 attachment / 崩溃重启       | `rustHostResilience.test.ts`                                         | 通过（既有）                                       |
| 无窗口调度                     | `automation_dispatch` 测试 + `rust-u2-automation.md` §5              | 通过（既有）                                       |
| 设置写入 / 单向读入            | `escode-settings-host-interop.test.ts` 两个用例                      | 通过                                               |
| 进程表（无 Node utility Host） | `conversation-session-rust-host-window-lifecycle.test.ts`            | 通过（artifact `desktop-e2e-20261023-092550-025`） |
| 远程 workspace                 | `conversation-session-ssh-remote-connection-p0.test.ts`（Rust 模式） | 通过（artifact `desktop-e2e-20261023-060839-165`） |

### 1.4 阶段 2 未完成项

- P1 #9：plugins crate 抽离与 `Ports` 注入。32 个 `plugin_*.rs`（7,290 行）仍与 `tools` 的
  `config_file` / `extension_*` / `mcp_config` / `atomic_dir` 相互依赖；抽 crate 需先下沉这些底座，
  属独立结构重构，未在本轮硬拆。

## 2. 阶段 3：切默认与清理

### 2.1 默认开关

`ESCODE_RUST_HOST` 未设置即为 Rust Host；显式 `0`/`false` 才回退 Node Host。依据四平台安装包 CI
run `39920171658`（四个 `installed` 作业的 Rust Host 启动步骤全绿）与默认翻转后的
run `39932318894`（结论 success）。远程 workspace 仍由窗口内 Node Host 服务（H2.3 边界），
Rust Host 不承载远程 scope。

### 2.2 installed 切换 + 回退演练

`scripts/ci-install-desktop.mjs` 新增 `--host-switch-drill`：

1. 写入冻结设置夹具（含未知键）到同一个 home；
2. 依次以 `ESCODE_RUST_HOST=0` → `1` → `0` 启动同一个安装包（同一 cwd/home）；
3. 每次断言数据库启动 ready 与 Host 类型（Node utility Host / Rust Host + renderer attach）；
4. 每次启动后校验设置文件仍含原 locale 与未知键。

`.github/workflows/release.yml` 的 `installed` 矩阵在 Rust Host 启动步骤后执行该演练。
本机用假应用验证了控制流与断言（三次启动分别判定 Node Host / Rust Host / Node Host，settings 夹具保留），
但**真实安装包的四平台 CI 尚未实跑**，因此阶段 3 该条保持未勾选。

### 2.3 golden freeze

- `scripts/diff-escode-cli-rust-parity.mjs` 新增：
  - `--record`：把 Node 侧归一化观察写入
    `scripts/fixtures/parity/<scenario>.<platform>-<arch>.node.golden.json`
    （system prompt 含 Platform/Shell，跨平台不可共用；无平台文件时回退通用名）；
  - `--golden`：不启动 Node，只跑 Rust 并对该平台 golden 判定。
- 本机验证：`--record` 判定 parity；`--golden` 只跑 Rust 后判定 parity。
- 差分夹具修复：Node 以入口目录发现官方插件并自动 seed，Rust 需要
  `ESCODE_OFFICIAL_PLUGINS_BASE_DIR` 与插件宿主 env。夹具补齐后两侧 tools/messages 对齐，
  删除已失效的 `model-request-messages-fixture-plugin-env` 批准条目。
- settings：`packages/services/tests/fixtures/settings-frozen.json` +
  `escode-settings-host-interop.test.ts` 的“冻结夹具 → Rust 单向读入”用例。
- **未完成**：165 个 CLI 差分用例与 25 个 desktop Host 用例的逐用例 Node/TS 观察冻结；
  在清零前不删除 Node Host / TS CLI。

### 2.4 回退矩阵与文档

- `docs/specs/rust-release-rollback.md` 新增「Host 实现切换矩阵」：本地 Rust Host / 远程 Node Host /
  Agent runtime 三个正交开关，含回退步骤与安装包演练命令。
- `AGENTS.md` 进程段落更新为：本地默认 Rust Host，远程 workspace 由窗口内 Node Host 服务。
- `CONTEXT.md` 无 Host 实现相关词条，无需修改。

## 3. 结论

阶段 2 的服务域迁移与验收项已完成，唯一未完成的是 P1 #9 的 plugins crate 结构重构。
阶段 3 已完成默认开关、六目标打包、回退矩阵与 golden freeze 的 Host parity/settings 切片；
剩余 installed 四平台演练实跑证据与 CLI/desktop 逐用例 golden 冻结。
