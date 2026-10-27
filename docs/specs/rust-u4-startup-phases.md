# Rust 阶段 4：冷启动阶段拆分（rust-unified-host.md §5「冷启动」）

父计划：`docs/specs/rust-unified-host.md` §阶段 4 第 3 项（冷启动 ≤1.8 s）。
相关：`rust-u4-startup-exit.md`（退出已交付）、`rust-u4-first-segment.md`（首段测量）。

## 1. 问题

安装包口径冷启动（`bench-escode-app` 的 C 配置）实测 2.4–2.6 s，目标 ≤1.8 s，但此前只有一个
总数，无法判断该优化 Host、主进程还是 renderer。

## 2. 测量契约

复用既有 launch marks 并补齐消费者：

- `desktopLaunchMarks` 已采集 T0（`process.getCreationTime()`）、T1（main JS 模块顶部）、
  T2（`app.whenReady`）。
- `desktopHostProcess.loadWindow` 注入 T3（loadURL 前）到主窗口 query。
- renderer `main.tsx` 记录 T4（bundle 开始执行）、T5（`StartupReadyNotifier` 的 React 首次 commit）。
- `ESCODE_BENCH_READY_LOG=1` 时主进程给窗口 URL 加 `benchReady=1`，renderer 输出一行：
  `[bench-startup] processToMain=… mainToAppReady=… appReadyToLoadUrl=… loadUrlToRenderer=…
rendererToReactCommit=… totalToReactCommit=…`
- `bench-escode-app` 解析该行写入 `summary.json.startupPhases` 与报告。生产不输出该日志。

## 3. 结果（Linux Xvfb，安装包 linux-x64，2026-10-28）

应用链路（C 配置，2 次运行一致）：

| 阶段                                     |        耗时 |
| ---------------------------------------- | ----------: |
| T0→T1 进程创建 → main JS                 |     1690 ms |
| T1→T2 main JS → app ready                |      313 ms |
| T2→T3 app ready → loadURL                |      179 ms |
| T3→T4 loadURL → renderer bundle 开始执行 |     1002 ms |
| T4→T5 bundle → React 首次 commit         |       25 ms |
| **T0→T5 总计（可交互）**                 | **3209 ms** |

基线探针（同一机器，裸 Electron）：

| 探针                                               |    耗时 |
| -------------------------------------------------- | ------: |
| 空 main（仅打印 T0→T1）                            | 1008 ms |
| 空页面 `loadURL` → `did-finish-load`（无 preload） |  100 ms |
| 同上，加载 `out/preload/index.cjs`（689 KB）       |  293 ms |

结论：

1. **T0→T1 的 1690 ms 中，1008 ms 是 Electron 自身启动地板**，应用 main bundle 增加约 680 ms。
2. **T3→T4 的 1002 ms 中**，空页面地板约 100 ms、preload 约 190 ms，其余约 700 ms 是
   renderer HTML/module graph 从 asar 加载与执行（初始入口 25 KB，但 modulepreload 图较大）。
3. bench 的「ready」是 `did-finish-load`（约 2.6 s），比 React 首次 commit（3.2 s）早约 600 ms；
   **2.6 s 不是可交互时间**，冷启动报告以 T0→T5 为准。
4. 1.8 s 目标在本 Linux/Xvfb 环境不可达：地板（Electron 1.0 s + 空页面 0.1 s + preload 0.2 s）已
   约 1.3 s，剩余预算不足以覆盖 main bundle 与 renderer 模块图。macOS M1 的 Electron 地板更低，
   需要在那台环境复测后才能对照 1.8 s。

## 4. 后续优化清单（未做，按收益排序）

1. main bundle 懒加载：把 index.ts 静态 import 的重依赖（services/provider/plugin 等）改为按需
   动态 import；目标削减 T0→T1 中应用侧约 680 ms 的一半以上。风险：启动期需要的能力必须保持
   同步可用（窗口、settings、Host 启动）。
2. preload 拆分/瘦身：`out/preload/index.cjs` 689 KB，单次加载约 190 ms；先拆出首屏不需要的
   bridge，再评估动态 require。
3. renderer 初始 module graph：收缩 modulepreload 链与首屏 chunk（当前有 5.6 MB styles chunk 等），
   目标削减 T3→T4 中的应用侧约 700 ms。
4. 以上三项需要分别 profile + 回归（首屏白屏风险高），不在本切片内实施。

## 5. 验收边界

- 本切片只交付测量：阶段日志、bench 解析、报告。
- 不改变窗口创建顺序、bootstrap 设置读取或渲染入口；不调整 Electron 启动参数。
- `did-finish-load` 仍是 bench 的就绪判定（用于"进程起来"），但冷启动报表同时给出 T0→T5。
