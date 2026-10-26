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

### 1.4 阶段 2 结构遗留（已闭合）

P1 #9：plugins crate 抽离与 `Ports` 注入（2026-10-23 完成）。

- 新建 `apps/escode-cli-rust/crates/plugins`（`escode-cli-plugins`），迁入 32 个 `plugin_*.rs`
  与底座 `config_file`/`atomic_dir`/`lexical_path`/`extension_config`/`extension_plugins`/
  `mcp_config`/`mcp_config_order`/`official_plugins*`/`bundled_skills`，以及内置 MCP 服务器
  `mcp_rea`/`mcp_serial`/`mcp_node_repl`（原先会让 `tools` 与插件域形成环）。
- `crates/plugins/src/ports.rs` 提供 `Ports`：统一取消语义与下载客户端，默认实现只依赖
  `escode-cli-core-api` / `escode-cli-net`；插件域不再引用 `tools`。
- `tools` 通过 `pub(crate) use escode_cli_plugins::{...}` 保持既有 `super::<module>` 调用面，
  只反向依赖插件 crate；`tool_port_plugins` 作为插件面的唯一入口。
- 验证：`cargo check --workspace --all-targets`、`cargo test --workspace --no-fail-fast`、
  `pnpm check:escode-cli-rust`（边界 + fmt + clippy `-D warnings`）全绿；
  `escode-cli-rust-plugins-options/list/describe/uninstall`、`bundled-skills` 差分用例通过。

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

2026-10-24 本地 linux-x64 证据：

- 用当前源码执行 `ESCODE_SKIP_REMOTE_ASSETS=1 pnpm bundle:desktop -- --os linux --arch x64`
  构建安装包；离线环境缺少 AppImage/deb 的 `app-builder` 下载资源，artifact 步骤失败，但
  electron-builder 已产出 `dist/linux-unpacked`（当前 `app.asar` +
  `resources/glm/escode-cli-rust` + `resources/escode-host/escode-host`），随后用
  `electron-builder --linux dir` 重建该目录。本地 drill 使用该安装布局；`.deb`/AppImage
  artifact 需在可联网的 release CI 生成。
- 首次真实启动发现两个打包/切换问题并已修复：
  1. `yazl` 的 hoisted 子依赖 `buffer-crc32` 未进 `app.asar`，主进程启动即
     `Cannot find module 'buffer-crc32'`；`bundle.mjs` 的运行时闭包加入 `yazl`，由
     `runtime-dependency-closure` 递归带入 `buffer-crc32`。
  2. Node Host 的 settings 写面按 schema 解析结果整体覆盖，未知键在 Node 启动后被丢弃，
     drill 的 `unknownFromNode` 校验失败；`settingService.writeSettings` 现在只保留
     原始文件中的未知顶层键（`appSettingsKnownKeys` 区分已知键），已知键仍按 patch 删除。
- 验证命令：
  `ESCODE_INSTALLED_APP=.../linux-unpacked/escode-preview ESCODE_INSTALLED_GLM=.../linux-unpacked/resources/glm node scripts/ci-install-desktop.mjs --launch-only --host-switch-drill`
  结果：`host switch and rollback drill passed: node → rust → node, settings preserved`。
- **四平台 release CI 实跑证据（2026-10-24）**：
  - run `40022683157`（https://github.com/sekfung/ESCode/actions/runs/40022683157，
    head `f5ef4608`）结论 **success**；`installed (win-x64 / mac-arm64 / mac-x64 / linux-x64)`
    全部 success。
  - 四个作业日志均包含
    `host switch and rollback drill passed: node → rust → node, settings preserved`，
    且每次 `launch drill 1/3 node`、`2/3 rust`、`3/3 node` 的 Host 类型断言通过。
  - `desktop (mac-arm64)` 首轮被 runner 取消（`bundle desktop` 中途），用
    `gh run rerun 40022683157 --failed` 重跑后 success；最终矩阵完整。
  - Windows 首次 drill 卡在 Node 模式：drill 与安装检查共用默认 `userData`，前一个实例的
    单实例锁尚未释放时 `requestSingleInstanceLock` 返回 false，`app.quit()` 后进程因
    crashReporter 等句柄不退出，数据库 startup 永远没有终态。drill 现在设置独立的
    `ESCODE_DESKTOP_USER_DATA_DIR` / `ESCODE_DESKTOP_SESSION_DATA_DIR`，并在每次启动前等待
    旧进程退出；Windows 诊断与四平台正式 run 均通过。
  - 同轮 `rust-runtime-ci` 的 cargo-deny 暴露 RUSTSEC-2026-0386；
    `tokio 1.53.1 → 1.53.3` 修复，checks 三平台恢复 success。

### 2.3 golden freeze

- `scripts/diff-escode-cli-rust-parity.mjs` 新增：
  - `--record`：把 Node 侧归一化观察写入
    `scripts/fixtures/parity/<scenario>.<platform>-<arch>.node.golden.json`
    （system prompt 含 Platform/Shell，跨平台不可共用；无平台文件时回退通用名）；
  - `--golden`：不启动 Node，只跑 Rust 并对该平台 golden 判定。
- 本机验证：`--record` 判定 parity；`--golden` 只跑 Rust 后判定 parity。
- CI：`rust-runtime-ci.yml` 的 linux-x64 差分步骤已切为 `--golden` 阻塞门禁；其余平台在录制
  对应平台 golden 前保留实时双跑（观察期）。
- 差分夹具修复：Node 以入口目录发现官方插件并自动 seed，Rust 需要
  `ESCODE_OFFICIAL_PLUGINS_BASE_DIR` 与插件宿主 env。夹具补齐后两侧 tools/messages 对齐，
  删除已失效的 `model-request-messages-fixture-plugin-env` 批准条目。
- settings：`packages/services/tests/fixtures/settings-frozen.json` +
  `escode-settings-host-interop.test.ts` 的“冻结夹具 → Rust 单向读入”用例。
- desktop Host oracle 已清零（2026-10-23/24，23 个测试文件 / 32 个测试）：
  - `rustHostRpc` / `rustHostTaskGroups` / `rustHostFileIndex` / `rustHostAttachments` /
    `rustHostWindowController` 使用冻结观察 + seed 库；
  - 其余 `rustHostAutomationParity`、`rustHostBuiltin*`、`rustHostClient*`、`rustHostCredential`、
    `rustHostDynamicWorkflow`、`rustHostFileWatcher`、`rustHostGit`、`rustHostHooksParity`、
    `rustHostPluginSyncParity`、`rustHostProvider*`、`rustHostSettingExtra`、`rustHostSkills`、
    `rustHostSvn`、`rustHostTerminal`、`rustHostUserConfig` 同样改为冻结观察；
  - 全部 `packages/desktop/test/rustHost*.test.ts` 不再 `import @escode/services` /
    `services/src/*`；资产在 `packages/desktop/test/fixtures/golden/`（大 JSON 用 gzip；
    seed 库在录制前快照）。23 个 `scripts/generate-rusthost-*-golden.ts --check` 汇总入口
    `pnpm check:rust-host-golden`。真实 `@escode/client` 保留（客户端契约，不是 oracle）。
- CLI oracle 已清零（2026-10-24）：`packages/services/tests/escode-cli-rust-*.test.ts` 共 156 个，
  正常跑只启动 Rust。
  - 110 个文件使用共享 `nodeOracle`（`escode-cli-rust-golden.ts`）record/load；观察
    `undefined` 数组元素归一为 `null`，提示词 `currentDate` 归一为 `<DATE>`，大观察 gzip；
    `ESCODE_RECORD_CLI_GOLDEN=1` 重录。
  - 其余文件中的 TS 源码 oracle（工具 schema/handler、凭据、提示词、迁移/导入 legacy DB）
    改为冻结 JSON 或 gzip SQLite seed（`escode-cli-rust-legacy-seed.ts`），不再静态 import TS 源码。
  - 例外（非确定性 parity 门禁）：`long-history-bench`、`real-data-rehearsal`、
    `real-provider-bench` 默认跳过并依赖真实 provider/数据库；`shell-resume-notice` 等仅
    Windows 启用的用例已由 record run `40040325646` 写入 `win32-x64` 平台 golden。
  - 冻结时的等价替代（原 Node 观察均保留在 golden）：凭据的 Node↔Rust 交叉解密改为冻结
    Node 密文 + Rust 解密/往返；automation/mcp-oauth 的 Node 写→Rust 读改为冻结 Node 观察
    - Rust 写读；migration/import 的 TS store 造数改为 gzip SQLite seed
      （`escode-cli-rust-legacy-seed.ts`，路径用占位符重写）。
- 平台专属 golden：record run `40040325646` 在 macos-latest / win32-x64 重跑 CLI 套件，
  `nodeOracle` 优先加载 `<suite>.<case>.<platform>-<arch>.json`（精确 arch 未命中时回退同 OS
  家族）；Linux 继续使用通用 golden，test 作业安装 `poppler-utils` 对齐 PDF 录制环境。
- 跨平台 CI：`rust-runtime-ci` run `40055867063`（head `63dee1c3`）**success**——checks/bench
  三平台、test 九个 shard（ubuntu/macos/windows）全部通过。macOS shard 1 的
  `file_watcher` 时序用例偶发失败，rerun 后 success（与 golden 无关）。
- **installed 四平台演练已完成**：release CI run `40022683157` 的四个 `installed` 作业全部
  success，node→rust→node 与 settings 保留断言均通过（见 2.2）。

### 2.4 回退矩阵与文档

- `docs/specs/rust-release-rollback.md` 新增「Host 实现切换矩阵」：本地 Rust Host / 远程 Node Host /
  Agent runtime 三个正交开关，含回退步骤与安装包演练命令。
- `AGENTS.md` 进程段落更新为：本地默认 Rust Host，远程 workspace 由窗口内 Node Host 服务。
- `CONTEXT.md` 无 Host 实现相关词条，无需修改。

## 3. 结论

阶段 2 的服务域迁移、验收项与 P1 #9 结构重构均已完成。
阶段 3 已完成默认开关、六目标打包、回退矩阵、Host parity/settings 切片、desktop Host 与
CLI oracle 的 golden freeze（含 macOS/Windows 平台专属 golden），以及 installed 四平台
Node→Rust→Node 真机演练（release run `40022683157` 四平台全绿，`settings preserved`）。
跨平台 golden CI：`rust-runtime-ci` run `40055867063` 九 shard + checks/bench 全绿。

## 4. 验收补齐（2026-10-26）

后续补齐记录见 `docs/reports/rust-stage3-acceptance-2026-10-26.md`：

- 安装包口径 A/B/C 交错测量已完成：C（Rust Host + 内嵌 runtime）冷启动 2.40 s、App RSS
  974.2 MiB、无 Node utility Host；A（Node Host + Node CLI）分别为 6.02 s / 1244.7 MiB / 2 个
  Node Host 进程。
- Linux parity golden 因 `currentDate` 未归一化而实际失效（录制次日即产生新差异），已修复并
  重新判定 parity；CI 改为按平台 golden 存在与否自动切换 `--golden` 阻塞门禁。
- **仍未闭合**：macOS/Windows parity golden 需一次对应 runner 的录制运行；跨平台桌面 E2E CI
  矩阵尚不存在；App 级首段 / 4 路流式 CPU / 多 workspace / 优雅退出需要新的会话驱动。
  因此本报告结论中的“阶段 3 已完成”应读作“主体已完成，验收未全部闭合”。
