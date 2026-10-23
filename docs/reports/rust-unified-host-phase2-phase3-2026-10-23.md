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
- **四平台 release CI 实跑证据仍待**（本机离线）；阶段 3 该条保持未勾选，直到
  `.github/workflows/release.yml` 的 `installed` 矩阵在可访问 GitHub 的环境运行通过。

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
    `real-provider-bench` 默认跳过并依赖真实 provider/数据库；`shell-resume-notice` 仅 Windows
    启用，golden 需在 Windows runner 重录一次。
  - 冻结时的等价替代（原 Node 观察均保留在 golden）：凭据的 Node↔Rust 交叉解密改为冻结
    Node 密文 + Rust 解密/往返；automation/mcp-oauth 的 Node 写→Rust 读改为冻结 Node 观察
    - Rust 写读；migration/import 的 TS store 造数改为 gzip SQLite seed
      （`escode-cli-rust-legacy-seed.ts`，路径用占位符重写）。
- **未完成**：installed 四平台 Node→Rust→Node 演练的 release CI 实跑证据（见 2.2）。

### 2.4 回退矩阵与文档

- `docs/specs/rust-release-rollback.md` 新增「Host 实现切换矩阵」：本地 Rust Host / 远程 Node Host /
  Agent runtime 三个正交开关，含回退步骤与安装包演练命令。
- `AGENTS.md` 进程段落更新为：本地默认 Rust Host，远程 workspace 由窗口内 Node Host 服务。
- `CONTEXT.md` 无 Host 实现相关词条，无需修改。

## 3. 结论

阶段 2 的服务域迁移、验收项与 P1 #9 结构重构均已完成。
阶段 3 已完成默认开关、六目标打包、回退矩阵、Host parity/settings 切片，以及
desktop Host 与 CLI 确定性 oracle 的 golden freeze（linux-x64 parity 已切 CI 阻塞门禁；
CLI 156 个文件正常跑只启动 Rust）；installed 演练已用当前源码重建的 linux-x64 包本地跑通
（`node → rust → node, settings preserved`），仅剩 release CI 四平台实跑证据。
