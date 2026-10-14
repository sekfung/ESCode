# App 级性能基线（阶段 0 验收产物）

日期：2026-10-14 ｜ 工具：`scripts/bench-escode-app.mjs` ｜ 原始数据：`.escode-runtime/app-bench-final/{summary.json,report-2026-10-14.md}`
计划出处：`docs/specs/rust-unified-host.md` 阶段 0 验收项「基线报告产出（记录 §5 全部指标）」。

## 一句话结论

开发检出（dev）口径下跑通三配置交错 A/B 基线：**B（Node Host + Rust CLI）App 常驻内存不高于 A（Node Host + Node CLI）**
（App 工作集 1293.6 vs 1419.6 MiB，私有提交 974.5 vs 1102.2 MiB，中位数），差额主要来自 Host utility（−71 MiB）
与 renderer（−21 MiB）；B 额外常驻/瞬态拉起 Rust CLI（约 115 MiB）。绝对数值属 dev 口径（vite dev 供版、
无 ASAR），与 §5 表里安装包口径（436 MiB/macOS、2.01 s）不可直接比较，安装包对照列入阶段 3。

## 口径与环境

- 机器：Windows 10.0.26100（x64）；Node v26.7.0；Rust release 二进制 `target/release/escode-cli-rust.exe`（cargo 1.95.0）。
- 配置：**A** = `pnpm dev:desktop`（Node Host + Node CLI，monorepo dev 默认）；**B** = `pnpm dev:desktop:escode-cli-rust`
  （Node Host + Rust CLI，显式 runtime override）。
- 场景：idle。就绪 = **electron 主进程与 renderer 同时在场**（按进程名判定，详见「重要修正」），再静置 20 s，
  采样 10 s（每 tick 全进程表一次，约 1–2 s 一 tick）。
- 交错 3 轮（A→B 每轮），取中位数；每轮隔离 data dir（fresh profile）；不建立任何调试/CDP 连接。
- 内存双口径（Windows）：WorkingSet64（工作集，footprint 近似）+ PrivateMemorySize64（私有提交）。
- **「全 App」= electron 主进程子树**（main/gpu/renderer/host×2/network/proxy/agent）；tsup/vite/esbuild/cmd/pnpm/node
  等 dev 构建链单列，不计入 App。CPU = 采样窗口内进程 CPU 时间差折算的单核百分比。

## 主结果（3 轮中位数）

| 配置          | 树合计工作集 MiB | **App 工作集 MiB** | **App 私有提交 MiB** | 树 CPU % | App CPU % | 就绪 s（dev 全链） | 退出回收 ms | 进程数 |
| ------------- | ---------------: | -----------------: | -------------------: | -------: | --------: | -----------------: | ----------: | -----: |
| A（Node CLI） |           3757.8 |         **1419.6** |           **1102.2** |     47.4 |      29.2 |              166.3 |        9702 |     28 |
| B（Rust CLI） |           3597.6 |         **1293.6** |            **974.5** |     50.9 |      26.2 |              167.5 |        9366 |     27 |

每轮原始值（树合计工作集 MiB / 就绪 s / 退出 ms）：

| 轮次 | A                     | B                     |
| ---- | --------------------- | --------------------- |
| 1    | 5581.2 / 168.7 / 9942 | 5082.2 / 168.4 / 9366 |
| 2    | 3681.9 / 166.3 / 9692 | 3466.3 / 164.9 / 9505 |
| 3    | 3757.8 / 162.9 / 9702 | 3597.6 / 167.5 / 9290 |

第 1 轮显著偏高（两个配置同时偏高，约 +1.5 GiB）：本轮为首启（包缓存/构建产物冷），属热失控项；
规则取中位数已剔除，但**跨机对比时必须同冷热状态**。

## 角色分解（3 轮中位数；App 归属以 electron 子树为准）

| 配置 | 角色                             | 归属   | 进程数 | 工作集 MiB | 私有提交 MiB |
| ---- | -------------------------------- | ------ | -----: | ---------: | -----------: |
| A    | main                             | App    |      1 |      189.8 |        136.9 |
| A    | gpu                              | App    |      1 |      154.7 |         49.9 |
| A    | renderer                         | App    |      1 |      508.4 |        511.5 |
| A    | host（node.mojom utility）       | App    |      2 |      408.1 |        330.9 |
| A    | network                          | App    |      1 |       68.7 |         21.8 |
| A    | proxy                            | App    |      1 |       56.1 |         16.0 |
| A    | devtool（esbuild --service）     | 非 App |      2 |      925.3 |        938.3 |
| A    | node/pnpm/shell/conhost          | 非 App |     19 |     1342.0 |       1208.0 |
| B    | main                             | App    |      1 |      188.8 |        137.6 |
| B    | gpu                              | App    |      1 |      146.6 |         49.1 |
| B    | renderer                         | App    |      1 |      487.3 |        486.4 |
| B    | host（node.mojom utility）       | App    |      2 |      341.7 |        264.1 |
| B    | network                          | App    |      1 |       62.2 |         15.6 |
| B    | proxy                            | App    |      1 |       55.9 |         15.0 |
| B    | **agent（escode-cli-rust.exe）** | App    |      1 |      114.9 |         93.0 |
| B    | devtool/node/pnpm/shell/conhost  | 非 App |     19 |     1273.0 |       1116.9 |

要点：

- **Host utility（node.mojom）在 B 下小 71 MiB（408.1→341.7）**——与「Rust runtime 承接了存储准备/运行期物化、
  Host 侧少一份对等状态」的预期方向一致（待阶段 2 分域后复测归因）。
- **Rust CLI（agent）在两个方向都出现**：3 轮中位数显示常驻约 115 MiB，但末次运行的末次采样里该进程已不在
  （27 个进程中无 agent）——即 Rust CLI 是**按需拉起/存储准备后退出**的形态；Node CLI 在空闲态则完全不拉进程。
- renderer 在 dev 下由 vite dev 供版（模块图未打捆）→ 487–570 MiB，是 App 内存最大单项，也是 dev 口径与
  安装包口径差距的主要来源。
- App CPU 26–29 %：采样窗口紧接 renderer 出现（静置 20 s），仍含首启工作（插件缓存、首屏），
  **不代表稳定空闲态**；下一轮应收紧为「预热 profile + 更长静置」。

## §5 指标逐项记录（本机 dev 口径）

| #   | §5 指标                      |                                                    本机实测（dev） | 与 §5 初始目标对照       | 说明                                                                                                                                                    |
| --- | ---------------------------- | -----------------------------------------------------------------: | ------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | 空闲内存（全 App）           | **A 1419.6 / B 1293.6 MiB**（App 工作集；私有提交 1102.2 / 974.5） | 无法直接对照（口径不同） | §5 的 436 MiB 是 macOS footprint + 安装包；本项为 Windows dev 检出。**阶段 3 在安装包上按同工具复测才可对照**                                           |
| 2   | 每多一个 workspace           |                                                             未覆盖 | —                        | 需 N=1/4/8 workspace 驱动场景；接入条件：阶段 1 `embed::spawn` 提供多实例后，在 harness 增加 workspace 打开命令                                         |
| 3   | 冷启动到主界面可用           |                            未覆盖（仅测到 dev 全链就绪 166–168 s） | —                        | 现就绪判定是「electron 主进程 + renderer 在场」，含 dev 构建链约 165 s，**不能当冷启动**；需日志级「主界面可用」信号（阶段 1 接入），安装包口径在阶段 3 |
| 4   | 首条消息第一段               |                                                             未覆盖 | —                        | 需驱动一条消息（mock SSE + 真实 provider 双测）；接入条件：阶段 1 会话驱动                                                                              |
| 5   | 退出（8 workspace 全部结束） |                                 未覆盖（仅测到强制回收 9.4–9.7 s） | —                        | 现值=taskkill /T /F 整树回收（27–28 进程），非优雅退出；8 workspace 优雅退出需多 workspace 场景                                                         |
| 6   | 进程内控制 RPC p50           |                                                             未覆盖 | —                        | 阶段 4（MemoryLink 免序列化）落地后才有对应链路                                                                                                         |
| 7   | 4 路流式全进程 CPU           |                   未覆盖（空闲 App CPU 29.2 % / 26.2 %，仅作参考） | —                        | 需 4 路并发流式负载；接入条件：阶段 1 会话驱动 + 流式压测脚本                                                                                           |
| 8   | 4 路流式 Host+network CPU    |                                                             未覆盖 | —                        | 同上；分进程采样基础设施已具备（角色分解 + CPU 差值）                                                                                                   |

## 重要修正记录（本基线前三次尝试作废的原因）

1. 原就绪判定用「进程树里出现含 `electron` 字样的命令行」——dev 的 `concurrently -k -n tsup,vite,electron …`
   **参数里就含 "electron"**，导致在 t≈118–121 s（构建链刚起）就判成就绪，实际采到的是 tsup/vite/esbuild
   构建链（三个"定版候选"轮次的数据因此全部作废，未采用）。
2. 探针实测：dev 流程里 **electron 主进程约 t≈173 s 才出现**（tsup/vite 就绪后 `dev.mjs` 才 launch）。
3. 修复：就绪判定改按**进程名**（`electron.exe` 主进程 + `--type=renderer` 子进程都在场）；角色分类同步改为
   按进程名判定 Electron，并新增 `host/network/proxy/devtool` 角色；「全 App」口径改为 electron 主进程子树。
4. 修复后自测（`--self-test`）恒绿，真实 A/B 复跑 3 轮得到上表数据。

## 结论与后续

1. **B 组合不劣于 A**：App 常驻（工作集/私有提交双口径）B 均低于 A，且 B 还多背一个 Rust CLI 进程；
   Host utility 显著变小是主要来源。方向与迁移目标一致，**无「Rust 组合更重」的阻塞性证据**。
2. 阶段 0 指标承诺（§5）在本机口径下的可测部分已建档；未覆盖 5 项均为「需场景驱动/安装包」类，
   接入条件已在表内逐条写明；**阶段 3 需用同一工具在安装包上复测，才可与 §5 目标值对照**。
3. 下一轮基线（阶段 1 后）应收紧：预热 profile、延长静置、日志级就绪、N=1/4/8 workspace、4 路流式 CPU。
4. 复现：

```bash
node scripts/build-desktop-agent-cli.mjs
cargo build --release --locked --manifest-path apps/escode-cli-rust/Cargo.toml --bin escode-cli-rust
node scripts/bench-escode-app.mjs --configs A,B --reps 3 --out .escode-runtime/app-bench-final
node scripts/bench-escode-app.mjs --self-test        # 不启动桌面的管线自检
```
