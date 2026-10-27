# Rust 阶段 4：启动/退出路径优化（rust-unified-host.md 超越项 3）

父计划：`docs/specs/rust-unified-host.md` §阶段 4「启动/退出路径优化」。
父计划的三个目标：冷启动 ≤1.8 s、首段 ≤180 ms、退出 ≤0.2 s。

本切片先交付**退出**（实测有明确瓶颈），冷启动与首段保持现状并登记为后续切片。

## 1. 问题（实测定位）

`app-perf-workspaces` 用例的退出测量（`app.quit()` → 主进程消失）为 2.17 s。逐 owner 计时后：

| 阶段                                    |        耗时 |
| --------------------------------------- | ----------: |
| cron / cua-permission / remote-sessions |       ≤4 ms |
| **rust-host stop**                      | **2135 ms** |
| Electron 关闭窗口与进程收尾             |      ≈40 ms |

继续拆 Host 内部：`eof shutdown: surfaces=0ms instances=14ms residual=0`，即**Host 业务收口只用了
14 ms**，其余约 2.0 s 全部花在 `ControlLoop::close()` 的 2 s 超时上。

根因：一个每 250 ms 轮询的 network-capture 转发任务持有控制队列写句柄（`ControlWriter` 克隆）。
`close()` 先把最后一个句柄丢掉、再等泵任务看到队列关闭；但该克隆让队列永不关闭，泵只能等满
2 s 超时后被 abort。同类风险适用于任何长驻后台任务（生命周期转发、host-events sink 等）。

## 2. 变更

1. **控制泵显式收口**（`apps/escode-host/src/control.rs`）：`ControlLoop` 持有
   `CancellationToken`；`close()` 先释放 sink、丢弃写句柄、取消 token。泵在收到取消后先
   `try_recv` 把已入队帧（`stopped`、最后一批日志）写完再退出，然后以 500 ms 兜底等待/abort。
   语义从「所有克隆都释放才退出」改为「显式信号 + 排空」，不再依赖后台任务的退出时序。
2. **后台转发任务显式取消**（`apps/escode-host/src/main.rs`）：生命周期转发与 network-capture
   转发都 `select!` 在同一个取消 token 上；收口时 `stop_background_tasks` 取消并给每个任务
   500 ms 自行退出，超时 abort。capture 在取消后**再 flush 一次**，最后一小批捕获不会丢。
3. **实例并发收口**（`host-runtime/src/lib.rs`）：`Instances::shutdown` 从串行 close 改为
   `join_all` 并发 close；残余清单仍按 identities 顺序拼接，输出稳定。实例之间无关闭依赖。
4. **可观测性**：`prepareAppQuit` 为每个 owner 打一行 `finished in Nms`；Host EOF 路径输出
   `eof shutdown: surfaces=Nms instances=Nms residual=N`。均为每次退出一行，不增加常态噪音。

## 3. 契约

- 退出屏障仍等待「Host 进程退出」后才放行第二次 `app.quit`；不新增超时绕过。
- 控制队列的**已入队**帧必须写完；取消之后新产生的消息属于已取消的后台任务，允许丢弃。
- 实例收口仍是 cancel → drain（宽限 5 s）→ 残余清单；并发只改变调度，不改变单个实例语义。
- 残余清单非空时仍返回退出码 2（既有行为）。
- 不改变 `Desktop` 侧 `forceKillDelayMs` / `waitTimeoutMs` 兜底；它们仍是最后防线。

## 4. 验收

| 项               | 方法                                                              | 结果                                            |
| ---------------- | ----------------------------------------------------------------- | ----------------------------------------------- |
| 8 workspace 退出 | `app-perf-workspaces.perf.ts`（`app.quit()` → 主进程消失）        | 2177 ms → **108 / 110 ms**（两次）              |
| Host 内部收口    | Host 日志 `eof shutdown`                                          | surfaces 0 ms / instances 14–16 ms / residual 0 |
| 主进程侧等待     | desktop 日志 `rust-host finished in Nms`                          | 2135 ms → 52–55 ms                              |
| 无残留           | 上述 residual 字段 + 现有 `rustHostResilience` / 窗口生命周期用例 | 0                                               |
| 现有套件         | `pnpm test:escode-host`、`pnpm test:escode-cli-rust`、跨平台 E2E  | 见 CI                                           |

目标对照：§5 的「退出 ≤0.2 s」在本口径（8 workspace、真实 before-quit 清理）达成。

## 5. 非目标（保持现状）

- **冷启动 ≤1.8 s**：安装包口径当前 2.40 s（Rust Host）。需要 Host 预热/懒加载改造，单独切片。
- **首段 ≤180 ms**：App 级「点击→首段可见」当前 1.38 s（Rust），与 runtime 首字口径不同；
  需要按 runtime 首字重新测量后才能对照 180 ms。
- Windows 的 `forceKillDelayMs` 兜底策略不变。

## 6. 风险

| 风险                     | 对策                                                                 |
| ------------------------ | -------------------------------------------------------------------- |
| 取消后丢失控制帧         | 泵先排空已入队帧；`stopped` 在取消前入队；capture 取消时补一次 flush |
| 并发收口放大 CPU         | 每个实例只做 cancel+drain；实测总时长≈最慢实例，无超时               |
| 退出变快导致进程未收干净 | residual 必须为空；现有刚性与 E2E 进程断言守护                       |
| 后台任务永久阻塞         | 500 ms 自行退出窗口，超时 abort；不依赖无限等待                      |
