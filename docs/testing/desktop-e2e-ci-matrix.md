# 桌面端跨平台 E2E 矩阵（阶段 3 验收）

本文定义 `.github/workflows/desktop-e2e-matrix.yml` 的用例清单、判定口径与已知边界。
它与 `installed` 作业（安装包启动与 Host 切换演练）互补：后者证明安装包能起来并切换
Host，**不**证明 GUI 交互可用。

## 判定口径

- 只跑正式目录（`packages/desktop/test/e2e/` 下非 `manual-review/pending`）的 spec；
  默认 replay 模式，不依赖真实 provider。
- 三平台（ubuntu / windows / macos）各跑同一份清单，任一平台失败即矩阵失败。
- 覆盖「有明确 setup + action + assertion」的路径；仅仅启动进程不算覆盖。

## 用例清单

| #   | spec                                                                                      | 覆盖的验收点                                                                                          | 备注                                                         |
| --- | ----------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------- | ------------------------------------------------------------ |
| 1   | `./test/e2e/smoke.test.ts`                                                                | 默认启动进入可交互工作区：V4 session pane 与 composer 同时可见                                        | 无 provider；主界面可用性最小门禁                            |
| 2   | `./test/e2e/infrastructure/session-lifecycle.test.ts`                                     | 每轮 WebDriver current handle 绑定真实 renderer page target，URL 不是 `chrome-error://`               | 排除白屏/绑错窗口类假绿                                      |
| 3   | `./test/e2e/conversation-session/conversation-session-rust-host-window-lifecycle.test.ts` | 本地窗口只由一个 `escode-host-rust-host` 服务、无 Node utility Host；renderer reload 后 Host pid 不变 | 阶段 3 的 Host 拓扑断言；`ESCODE_RUST_HOST` 未设即 Rust 模式 |

命令（CI 与本地一致）：

```bash
ESCODE_E2E_SPEC='./test/e2e/smoke.test.ts,./test/e2e/infrastructure/session-lifecycle.test.ts,./test/e2e/conversation-session/conversation-session-rust-host-window-lifecycle.test.ts' \
  pnpm --filter @escode/desktop test:e2e:serial
```

`workflow_dispatch` 的 `specs` 输入可覆盖清单用于定位问题；矩阵验收仍以默认清单为准。

## App 级性能作业（app-perf）

同一 workflow 的 `app-perf` 作业在 ubuntu-latest 上跑
`./test/e2e/perf/app-perf-streaming.perf.ts`，对 Node 与 Rust 两种 Agent runtime 各跑一遍：

```bash
E2E_PROVIDER_REPLAY_FIXTURE_PATH=packages/desktop/test/e2e/fixtures/upstream/common.json,\
packages/desktop/test/e2e/fixtures/upstream/perf/app-perf-streaming.json \
ESCODE_APP_PERF_LABEL=node ESCODE_AGENT_SERVER_RUNTIME=node \
  xvfb-run -a pnpm --filter @escode/desktop exec wdio run wdio.conf.ts \
    --spec ./test/e2e/perf/app-perf-streaming.perf.ts
```

- `.perf.ts` 不进默认 E2E spec 发现，避免性能数字波动让功能套件变红。
- 测量项：4 路并发流式下的「点击 → 首段可见」延迟、Electron 主进程子树（含 Host 与 Agent）
  的 CPU/工作集，以及 `app.getAppMetrics()` 的 Electron 口径对照。
- 结果写入 `$ESCODE_E2E_ARTIFACT_DIR/app-perf-streaming.<runtime>.<platform>-<arch>.json`，
  作业 summary 输出 Node/Rust 对比表。当前只记录趋势，不做阈值判定。

## 覆盖矩阵（阶段 3 验收）

| 平台           | 桌面 GUI 可用             | Host 拓扑                         | App 级性能                                     |
| -------------- | ------------------------- | --------------------------------- | ---------------------------------------------- |
| ubuntu-latest  | smoke + session-lifecycle | Rust Host 单进程、reload pid 不变 | 首段 + 4 路流式（Node/Rust 各一次）            |
| windows-latest | smoke + session-lifecycle | 同上                              | —（读数只与同平台历史比较，暂只在 Linux 采集） |
| macos-latest   | smoke + session-lifecycle | 同上                              | —                                              |

**未覆盖**：多 workspace（N=1/4/8）与 8 workspace 优雅退出；Host/network 分角色 CPU。
这两项需要工作区种子 + UI 打开流程与分角色采样，尚未接入本 workflow。

## 不在本矩阵内

- `manual-review/pending` 的候选用例：需要人工 review 后才能转正。
- 需要真实 provider / 真实 SSH / 真实账号的用例。
- Docker `replay-isolated` suite：由 `test:e2e:container` 单独承接。

## 已知边界

- macOS runner 没有签名身份与 Gatekeeper 放行流程；E2E 使用未打包产物（`.e2e-cache`
  副本 + 仓库 Electron 二进制），不经过安装包签名路径。
- Linux runner 依赖 Xvfb（WDIO `autoXvfb` 默认开启），本 workflow 显式安装 `xvfb`。
- 矩阵在 Windows 上使用单实例 `test:e2e:serial`，不分片；分片入口
  （`test:e2e -- --shards=N`）仍用于全量回归，本矩阵只做跨平台冒烟与 Host 拓扑门禁。
