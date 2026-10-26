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

## 不在本矩阵内

- `manual-review/pending` 的候选用例：需要人工 review 后才能转正。
- 需要真实 provider / 真实 SSH / 真实账号的用例。
- App 级性能指标（首段、4 路流式 CPU、多 workspace、优雅退出）：需要会话驱动与采样，
  见 `docs/reports/rust-stage3-acceptance-2026-10-26.md` 的未闭合项。
- Docker `replay-isolated` suite：由 `test:e2e:container` 单独承接。

## 已知边界

- macOS runner 没有签名身份与 Gatekeeper 放行流程；E2E 使用未打包产物（`.e2e-cache`
  副本 + 仓库 Electron 二进制），不经过安装包签名路径。
- Linux runner 依赖 Xvfb（WDIO `autoXvfb` 默认开启），本 workflow 显式安装 `xvfb`。
- 矩阵在 Windows 上使用单实例 `test:e2e:serial`，不分片；分片入口
  （`test:e2e -- --shards=N`）仍用于全量回归，本矩阵只做跨平台冒烟与 Host 拓扑门禁。
