# Rust 阶段 4：退出路径优化（2026-10-26）

父计划：`docs/specs/rust-unified-host.md` §阶段 4 第 3 项。契约：`docs/specs/rust-u4-startup-exit.md`。

## 1. 结论

8 workspace 的优雅退出从 **2177 ms 降到 108 / 110 ms**（两次复测），达到 §5 的「退出 ≤0.2 s」。
Host 业务收口本身只需 14–16 ms；此前的 2 秒全部耗在控制队列的 2 s 超时上。

## 2. 定位过程

1. `app-perf-workspaces` 用例测到 `app.quit()` → 主进程消失约 2.17 s。
2. 给 `prepareAppQuit` 的每个 owner 加计时：cron/cua/remote ≤4 ms，`rust-host stop` 2135 ms。
3. 给 Host EOF 路径加计时：`surfaces=0ms instances=14ms residual=0` —— Host 业务收口很快。
4. 差值指向 `ControlLoop::close()`：它等泵任务看到「队列关闭」，而一个每 250 ms 轮询的
   network-capture 转发任务持有写句柄克隆，队列永不关闭 → 等满 2 s 超时后 abort。

## 3. 变更

| 文件                                 | 变更                                                                                                                       |
| ------------------------------------ | -------------------------------------------------------------------------------------------------------------------------- |
| `apps/escode-host/src/control.rs`    | 控制泵增加 `CancellationToken`；`close()` 显式取消，泵先排空已入队帧再退出，兜底 500 ms                                    |
| `apps/escode-host/src/main.rs`       | 生命周期/capture 转发任务响应同一 token；收口时取消并等待（500 ms/任务）；capture 取消后补一次 flush；EOF 路径输出阶段计时 |
| `host-runtime/src/lib.rs`            | `Instances::shutdown` 并发 close（`join_all`），残余清单保持 identities 顺序                                               |
| `packages/desktop/src/main/index.ts` | 退出屏障逐 owner 计时日志（每次退出一行）                                                                                  |

未改变：退出屏障仍需等 Host 进程退出；实例 cancel→drain(5 s)→残余清单语义；`forceKillDelayMs`
/`waitTimeoutMs` 兜底；残余非空时退出码 2。

## 4. 实测

本地 Linux Xvfb，`app-perf-workspaces.perf.ts`（8 workspace、每个跑一轮会话）：

| 指标                                        |  优化前 |   优化后（两次） |
| ------------------------------------------- | ------: | ---------------: |
| `app.quit()` → 主进程消失                   | 2177 ms | **108 / 110 ms** |
| desktop 侧 `rust-host stop`                 | 2135 ms |       52 / 55 ms |
| Host `eof shutdown`（surfaces / instances） |       — |  0 ms / 14–16 ms |
| residual                                    |       0 |                0 |

## 5. 验证

- `pnpm check:escode-host`：boundary + fmt + clippy `-D warnings` exit 0。
- `pnpm test:escode-host`：59 个 suite 全绿（含实例表/崩溃恢复/窗口 Host 用例）。
- `pnpm test:escode-cli-rust`：374 pass / 0 fail（CI 同口径 serial + 60 s 预算）。
- `pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed`：通过。
- CI：`rust-runtime-ci`（含三平台 golden 门禁）与 `desktop e2e matrix`（三平台 + app perf）。

## 6. 未完成（阶段 4 第 3 项剩余）

- **冷启动 ≤1.8 s**：安装包口径当前 2.40 s；需要 Host 预热/懒加载，单独切片。
- **首段 ≤180 ms**：App 级「点击→首段可见」当前 1.38 s，口径与 runtime 首字不同；需要先补
  runtime 首字测量再对照。
- 本次只优化退出；未引入预热或快照式刷盘。
