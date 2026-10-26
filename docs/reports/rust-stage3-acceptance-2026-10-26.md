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
| 退出（8 workspace 全部结束） | 未测优雅退出；强制回收 66–73 ms             | 需要 UI 驱动关闭 8 workspace；当前只测整树 SIGKILL                                               |
| 每多一个 workspace           | 未测                                        | 需要 N=1/4/8 workspace 驱动                                                                      |
| 首条消息第一段               | 未测（App 级）                              | 需要 mock SSE + UI 会话驱动                                                                      |
| 4 路流式全进程 CPU           | 已测（App 级，见 §5.1）                     | Linux Xvfb：Node 167.4% / Rust 144.3%（中位数）                                                  |
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
对比表写进 job summary；阈值判定尚未引入（先积累趋势）。

## 6. 仍未闭合的阶段 3 项

1. **多 workspace（N=1/4/8）与 8 workspace 优雅退出**：需要工作区种子 + UI 打开流程；当前
   E2E 只提供 `seedPassiveAgentStartupWorkspaces`（建目录）与侧栏打开 helper，尚未接入 perf
   作业。指标口径与接入方式见 §4 表。
2. **4 路流式的 Host/network 分角色 CPU**：全进程口径已测，未按角色拆分（§5 的 ≤4 个百分点
   是分进程口径）。
3. **首段/CPU 的阈值判定**：目前只落盘数字与对比表，未设回归阈值，避免 runner 抖动误红。

结论：阶段 3 的安装包性能对照、三平台 parity golden 阻塞门禁、跨平台桌面 E2E 矩阵与 App 级
首段/4 路流式 CPU 已完成并实测；剩余 1–3 属于口径细化与阈值治理，不再阻塞「Rust Host 默认
可用」的验收结论。多 workspace 与优雅退出仍需单独接入后才能勾选。
