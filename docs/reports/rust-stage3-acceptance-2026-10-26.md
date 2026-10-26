# Rust 统一 Host：阶段 3 验收补齐（2026-10-26）

父计划：`docs/specs/rust-unified-host.md` §阶段 3。本文只记录本轮补齐的验收证据、修正的测量口径
与仍未闭合项；不把“已实现”写成“已验收”。

## 1. 环境与构建

- 源码：`dev` @ `a385da96afe4f7014f323c5152baddb7a6fbb3a8`（v3.15.2 + 发布后 3 个修复）。
- 机器：Linux `7.0.0-38-generic` x64，Intel Xeon Gold 5218R @ 2.10 GHz（16 vCPU），无
  `DISPLAY`，经 `xvfb-run` 启动；Node `v24.14.0`，Electron `41.0.3`。
- 安装包：`ESCODE_SKIP_REMOTE_ASSETS=1 pnpm bundle:desktop -- --os linux --arch x64`。
  离线环境缺少 AppImage/deb 的 `app-builder` 资源，artifact 步骤失败，但 `dist/linux-unpacked`
  已产出（与 2026-10-24 本地演练同一路径）；测量只使用 `linux-unpacked`。
- 被测资产（sha256）：测量后仅把 `[bench-ready]` 的 `logger.info` 改成可选链以修正类型，
  行为不变，未重新打包。
  - `app.asar` `c80fbe17ab7ff4cc6244c8f050325745113e394272a6fda9a86e7c40f54e91cc`
  - `resources/escode-host/escode-host` `843dd5810a1c2f4c20cc961607958bc898f8727826ada17993528518a2889bc6`
  - `resources/glm/escode-cli-rust` `8eced16816da9d0fa73f6689df9bb861abb607bf6d35b1613d04f8321a58fb39`
  - `resources/glm/zcode.cjs` `be0734b5cb2a6e1f7bd0128a6a0c47a8d2f627356efd5b1bc0a87be9b1a0e2fa`

## 2. 测量方法

命令：

```bash
node scripts/bench-escode-app.mjs \
  --installed-app packages/desktop/dist/linux-unpacked/escode-preview \
  --installed-glm packages/desktop/dist/linux-unpacked/resources/glm \
  --configs A,B,C --reps 3 --settle 20 --sample 10 --cooldown 5 \
  --out .escode-runtime/app-bench-installed-2026-10-26
```

配置（同一个安装包内只切换 Host/runtime，避免 dev 构建链计入冷启动）：

| 配置 | Host      | Agent runtime | 关键 env                                                            |
| ---- | --------- | ------------- | ------------------------------------------------------------------- |
| A    | Node Host | Node CLI      | `ESCODE_RUST_HOST=0`、`ESCODE_AGENT_SERVER_RUNTIME=node`            |
| B    | Node Host | 包内 Rust CLI | `ESCODE_RUST_HOST=0`、`ESCODE_AGENT_SERVER_RUNTIME=escode-cli-rust` |
| C    | Rust Host | 内嵌 runtime  | `ESCODE_RUST_HOST=1`、`ESCODE_HOST_RUNTIME_LINK=embedded`           |

- 就绪：主进程日志 **同时** 出现 `[database-startup] terminal … status: 'ready'` 与
  `[bench-ready] main window did-finish-load`（`ESCODE_BENCH_READY_LOG=1` 时输出，生产默认关闭）。
  比进程名轮询更接近「主界面可用」；仍不是 React 数据加载完成信号。
- 每轮隔离 `HOME`/`XDG_*`/`dataBaseDir`/Electron userData/sessionData；3 轮交错、取中位数。
- App 口径 = 主进程子树；安装包主进程 argv 被 `process.title` 改写，按 zygote 的 ppid 反查主进程。
  `chromium` 是 Electron 41 下 argv 仍显示 `--type=zygote` 的 renderer/辅助进程。
- CPU：Linux `/proc/<pid>/stat` 的 jiffies（100 Hz），单核百分比；`ps -o time=` 的秒级分辨率
  已不再用于 Linux。
- 退出：整树 `SIGKILL` 回收，衡量的是回收耗时，不是 8 workspace 优雅退出。

## 3. 结果（3 轮中位数）

| 配置                       | 冷启动 s | 树 RSS MiB | App RSS MiB | App CPU % | 退出回收 ms | 进程数 |
| -------------------------- | -------: | ---------: | ----------: | --------: | ----------: | -----: |
| A Node Host + Node CLI     |     6.02 |     1324.5 |      1244.7 |      36.8 |          71 |     10 |
| B Node Host + Rust CLI     |     3.81 |     1373.3 |      1293.5 |      37.4 |          73 |     10 |
| C Rust Host + 内嵌 runtime | **2.40** | **1054.0** |   **974.2** |  **33.8** |          66 |      9 |

角色分解（RSS MiB）：

| 配置 |  main | chromium |                            host |                       host-rust | network | 非 App（xvfb-run/Xvfb） |
| ---- | ----: | -------: | ------------------------------: | ------------------------------: | ------: | ----------------------: |
| A    | 279.4 |    544.6 | 336.5（Node Host + node.mojom） |                               — |    84.5 |                    79.8 |
| B    | 279.8 |    544.6 | 384.8（Node Host + node.mojom） |                               — |    83.9 |                    79.8 |
| C    | 280.0 |    542.6 |                               — | 70.1（Rust Host，内嵌 runtime） |    84.1 |                    79.8 |

相对 A：

- C 冷启动 −60.1%，App RSS −21.7%，App CPU −8.2%，进程数 −1。
- C 的进程表只有 1 个 `escode-host`（`host-rust`），**没有** Node utility Host（`host`）；
  A/B 各有两个 `host`（`escode-host-local-*` 与 node.mojom utility）。
- B 在 idle 场景不会拉起 agent 进程，B/A 差异只反映 Host/Electron 侧；Rust CLI 的实际流式开销
  见 §4 的运行时级参考。
- 三配置的空闲 CPU 都在 33–37%，与旧基线 dev 口径的 26–29% 同量级或更高；这是安装包 + Xvfb +
  空 profile 下的总 App CPU，尚未归因到具体角色，不能当作“流式 CPU”验收。

## 4. 验收项映射

| 指标                         | 本轮结果                                    | 说明                                                                                             |
| ---------------------------- | ------------------------------------------- | ------------------------------------------------------------------------------------------------ |
| 冷启动到主界面可用           | 已测：A 6.02 s / B 3.81 s / C 2.40 s        | §5 的 ≤1.8 s 是 macOS footprint 口径，Linux/Xvfb 数值不可直接对照；同口径下 C 明显优于 Node 组合 |
| 空闲内存（全 App）           | 已测：A 1244.7 / B 1293.5 / C 974.2 MiB RSS | §5 的 ≤430 MiB 是 macOS footprint；Linux RSS 口径不可直接对照                                    |
| 退出（8 workspace 全部结束） | 已测（App 级，见 §5.2）                     | `app.quit()` 到主进程消失（含 before-quit 清理）：Node 2179 ms / Rust 2174 ms                    |
| 每多一个 workspace           | 已测（App 级，见 §5.2）                     | N=1..8 八点回归，Host+Agent 子集：Node 10.44 / Rust 11.49 MiB/ws；App 树受 renderer GC 影响为负  |
| 首条消息第一段               | 已测（App 级，见 §5.1）                     | Linux Xvfb CI：Node 1772.5 ms / Rust 1417.5 ms                                                   |
| 4 路流式全进程 CPU           | 已测（App 级，见 §5.1）                     | Linux Xvfb CI：Node 134.8% / Rust 109.0%（中位数）                                               |
| 4 路流式 Host+network CPU    | 已测（含在 §5.1 的全进程树口径）            | 未单独拆分 Host/network 角色                                                                     |
| 进程内控制 RPC p50           | 未测                                        | 阶段 4（MemoryLink 免序列化）                                                                    |
| 进程布局                     | 已测                                        | C 单 Rust Host、无 Node utility Host；A/B 与预期一致                                             |

运行时级流式参考（`scripts/bench-escode-cli-node-rust.mjs`，1 会话 × 8 轮 × 256 chunk，
本地 3 次中位数；不是 App 级口径）：

| runtime  | 启动 ms | 空闲 RSS MiB | 峰值 RSS MiB | 8 轮 wall s |
| -------- | ------: | -----------: | -----------: | ----------: |
| Node CLI |    1641 |          353 |          456 |        7.97 |
| Rust CLI |     858 |         17.5 |         36.6 |        1.12 |

该脚本仍用 `ps -o time=`，Rust 侧 CPU <1 s 被记为 0，不能用于 CPU 百分比结论；只作流式处理
开销与内存的量级参考。

## 5. parity golden 门禁

本轮发现并修复了 Linux `--golden` 门禁的实际失效：

- 冻结观察里含 `Today's date is 2026-10-24.`，归一化没有覆盖 `currentDate`，录制次日开始
  每天产生新差异。已在 `scripts/diff-escode-cli-rust-parity-diff.mjs` 增加 `current-date` 规则
  （理由随 evidence 输出），`--golden` 重新判定为 parity。
- `--record` 原先写入通用文件名 `session-baseline.node.golden.json`，平台门禁永远看不到该
  golden；现在 `--record` 必写 `<platform>-<arch>` 文件。
- 欠账指纹按平台分文件（`scripts/diff-escode-cli-rust-parity.baseline.<platform>-<arch>.json`）：
  Windows 的帧批次与 seq 编号与 Linux 不同（T-4 投递层欠账），共享基线会让新平台每帧都判
  「新差异」。判定优先平台文件，缺失时回退通用文件（linux-x64 现状）。
- `rust-runtime-ci.yml` 按平台 golden 存在与否自动选择：有 golden → `--golden` 阻塞门禁；
  无 golden → Node/Rust 实时双跑观察期并输出录制提示。
- `record-cli-goldens.yml` 流程：录 golden → 冻结本平台欠账基线 → Rust-only `--golden` 验证 →
  上传 golden 与基线两个 asset。不把实时双跑作为前置条件（帧批次顺序会间歇性产生新差异）。

**已闭合（2026-10-26）**：`record cli goldens` 在 macOS/Windows runner 上录制并验证通过
（run `40281946953` 两个作业 success），产物已入库：

- `scripts/fixtures/parity/session-baseline.darwin-arm64.node.golden.json`（53 frames，录制于
  2026-10-26T06:52:37Z）
- `scripts/fixtures/parity/session-baseline.win32-x64.node.golden.json`（53 frames，录制于
  2026-10-26T06:55:54Z）
- `scripts/diff-escode-cli-rust-parity.baseline.darwin-arm64.json`（10 条欠账指纹）
- `scripts/diff-escode-cli-rust-parity.baseline.win32-x64.json`（9 条欠账指纹）

三个平台的 golden 落库后，`rust-runtime-ci` 的 parity 步骤在 macOS/Windows 也会自动切到
`--golden` 阻塞门禁，不再依赖 Node 实时双跑。

## 5.1 App 级首段与 4 路流式 CPU

新增测量用例 `packages/desktop/test/e2e/perf/app-perf-streaming.perf.ts`（`.perf.ts` 不进默认
E2E 发现，只在显式 `--spec` 或 CI perf 作业运行）。负载：4 个会话并发，每条 lane 约 12s 的
固定 SSE 流（`fixtures/upstream/perf/app-perf-streaming.json`），期间每 500ms 采样：

- 首段 = 点击发送到该 lane 首个 assistant 文本出现在时间线的间隔；
- 全进程 CPU/内存 = Electron 主进程子树（含 Host 与 Agent，Linux 用 `/proc` jiffies，
  Windows 用 CIM），同时记录 `app.getAppMetrics()` 的 Electron 口径作对照。

本地 Linux Xvfb（2026-10-26，dev 构建，各 1 次）：

| Agent runtime | 首段中位数 ms | 首段最大 ms | 全进程 CPU 中位数 | 全进程 CPU 峰值 | 全进程工作集中位数 MiB | Electron CPU 中位数 |
| ------------- | ------------: | ----------: | ----------------: | --------------: | ---------------------: | ------------------: |
| Node CLI      |        1545.5 |        1764 |            167.4% |          276.2% |                 1261.5 |                6.3% |
| Rust CLI      |        1533.5 |        1672 |            144.3% |          234.5% |                 1256.6 |                6.2% |

结论：同一负载下 Rust runtime 的首段与 Node 持平（差 0.8%，单次运行内噪声级），全进程 CPU
中位数低 13.8%，峰值低 15.1%，工作集持平。绝对值高于 §5 的 236 ms 目标，因为这里测量的是
「点击 → 首段可见」的端到端口径（含提交往返与 markdown 渲染），不是 runtime 首段吐字口径；
两者不可直接对照。

CI 作业 `desktop-e2e-matrix.yml` 的 `app-perf` 会在 ubuntu-latest 上跑 Node 与 Rust 两遍并把
对比表写进 job summary。CI 实测（PR run `40287649972`，ubuntu-latest，各 1 次）：

| Agent runtime | 首段中位数 ms | 首段最大 ms | 全进程 CPU 中位数 | 全进程 CPU 峰值 | 全进程工作集中位数 MiB | 进程数 |
| ------------- | ------------: | ----------: | ----------------: | --------------: | ---------------------: | -----: |
| Node CLI      |        1772.5 |        1932 |            134.8% |          301.5% |                 1193.4 |     10 |
| Rust CLI      |        1417.5 |        1466 |            109.0% |          211.1% |                 1207.2 |      7 |

CI 结论：Rust runtime 首段低 20.0%，全进程 CPU 中位数低 19.1%、峰值低 30.0%，工作集高 1.2%
（噪声级），进程数少 3。与本地两次运行的方向一致（Rust 首段与 CPU 不劣于 Node）。阈值判定
尚未引入，先积累趋势，避免 runner 抖动误红。

## 5.2 多 workspace 与优雅退出

新增测量用例 `packages/desktop/test/e2e/perf/app-perf-workspaces.perf.ts`。wdio 的
`APP_PERF_WORKSPACES_SPEC` 分支会在启动前播种 8 个已恢复 workspace（全部作为 tab 恢复，
`lastActiveTabIndex=0`）；用例逐个切换 workspace 并**真实跑一轮会话**（fixture
`E2E_APP_PERF_WS_<n>`），让该 workspace 的运行时实例常驻，再在 N=1..8 每个点取检查点。

只看草稿切换不建实例，首轮实测内存完全持平（1185 → 1183 MiB），测不到「每多一个 workspace」
的成本；因此口径改为「已跑过会话的 workspace 数」。

检查点从 3 个（1/4/8）加密到 8 个：三点斜率方差过大（Node 本地 6.43 vs CI 1.10 MiB/ws），
单次运行不足以支撑结论。8 点的最小二乘更稳，成本只多约 1 分钟。

本地 Linux Xvfb（2026-10-26，各 1 次，稳定 5s 后 5 次采样取中位数，N=1..8 八点回归）：

| Agent runtime | Host+Agent 斜率 MiB/ws | App 树斜率 MiB/ws | 端点差 MiB/ws | N=1 MiB | N=8 MiB | 退出 ms |
| ------------- | ---------------------: | ----------------: | ------------: | ------: | ------: | ------: |
| Node CLI      |                  10.44 |             -5.53 |         -7.17 |  1215.8 |  1165.6 |    2179 |
| Rust CLI      |                  11.49 |             -6.66 |         -5.54 |  1233.7 |  1194.9 |    2174 |

- **主指标是 Host+Agent 子集斜率**（§5「每多一个 workspace」的成本来源）；App 全进程树斜率
  为负，因为总 RSS 由 renderer 主导，第 3–4 个 workspace 出现 GC 回落（1219 → 1087 MiB），
  不能当作 per-workspace 成本，只保留观察总体趋势。
- Host+Agent 子集（`escode-host*` / `escode-cli-rust` / `zcode.cjs` / `app-server`）随
  workspace 单调增长：Node 141.1 → 217.6 MiB，Rust 逐点递增到同量级；斜率 10.4–11.5 MiB/ws。
- 退出为真实 `app.quit()` 路径（before-quit 清理完成后再退），不是整树 SIGKILL；用
  `/proc/<pid>/stat` 状态判定，僵尸（Z）视为已退出，否则父进程未 reap 会让测量永远超时。
- 两侧同量级：Rust 的 Host+Agent 斜率略高（+10.0%）、退出略快（−0.2%），均在单次运行波动内，
  不能据此判定 Rust 更差或更优；需要 CI 多次趋势才能下结论。

与 §5 初始目标的差距（如实记录，不调口径凑数）：

- §5「每多一个 workspace ≤2.5 MiB」当前未达到：Host+Agent 子集 10.4–11.5 MiB/ws。该目标的
  原意是对标 ZCode-rs 的扩展/配置缓存增量，而本测量包含每个 workspace 的会话运行时状态
  （8 个 workspace 各跑过一轮会话），口径更宽；两者不能直接对上。要判定 §5 目标是否达成，
  需要 Host 侧暴露「每实例扩展/配置缓存」的计数，作为后续小工作包。
- §5「退出 ≤0.2 s」当前未达到：实测 2.17–2.18 s，耗时在 before-quit 的 Host 清理屏障
  （日志：`waiting for host process cleanup` → `completed` 约 2.05 s），不是 Electron 进程没收口。
  这是「优雅退出」的真实成本，优化点在于清理屏障本身（阶段 4「启动/退出路径优化」）。

## 5.3 CI 证据汇总

PR https://github.com/sekfung/ESCode/pull/2（37 项检查全绿，已合并）：

| 项目                | 运行                                           | 结论                                                                                      |
| ------------------- | ---------------------------------------------- | ----------------------------------------------------------------------------------------- |
| mac/win golden 录制 | `record cli goldens` run `40281946953`         | 两个作业 success，golden 与平台基线已入库                                                 |
| 三平台 golden 门禁  | `rust-runtime-ci` run `40287649913` 的 `bench` | ubuntu/macos/windows 均执行 `Rust parity against frozen golden` success，实时双跑 skipped |
| 跨平台桌面 E2E      | `desktop e2e matrix` run `40287649972`         | ubuntu/macos/windows 三个 `desktop e2e` 均 success                                        |
| App 级性能          | 同上 run 的 `app perf (ubuntu-latest)`         | success，产出 Node/Rust 的流式 JSON 与对比表                                              |

多 workspace 与优雅退出用例在本轮新增，CI 证据见后续 `app perf` 作业（步骤
`measure workspaces (node|rust runtime)` 与 summary 的「多 workspace 与优雅退出」表）。

## 6. 仍未闭合的阶段 3 项

1. **4 路流式的 Host/network 分角色 CPU**：全进程口径已测，未按角色拆分（§5 的 ≤4 个百分点
   是分进程口径）。
2. **首段/CPU/退出/workspace 的阈值判定**：目前只落盘数字与 CI 对比表，未设回归阈值，避免
   runner 抖动误红；需要先积累多次 CI 运行的趋势分布。
3. **§5 的 ≤2.5 MiB/workspace 与 ≤0.2 s 退出目标**：已实测但未达到，口径差异与优化点见 §5.2；
   需要 Host 侧缓存计数与退出屏障优化两个独立工作包，不在本轮范围内。

结论：阶段 3 的安装包性能对照、三平台 parity golden 阻塞门禁、跨平台桌面 E2E 矩阵、App 级
首段/4 路流式 CPU，以及多 workspace 与 8 workspace 优雅退出均已完成并实测（PR #2 的 CI 全绿）；
剩余 1–3 属于阈值治理与 §5 目标优化，不再阻塞「Rust Host 默认可用」的验收结论。
