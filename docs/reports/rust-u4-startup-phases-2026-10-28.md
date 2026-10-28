# Rust 阶段 4：冷启动阶段拆分结果（2026-10-28）

父计划：`docs/specs/rust-unified-host.md` §阶段 4 第 3 项。契约：`docs/specs/rust-u4-startup-phases.md`。

## 1. 结论

安装包冷启动 2.4–2.6 s 不是 Host 的问题：Host 启动只占 ~6 ms。拆分后：

| 阶段（T0 进程创建 → T5 React 首次 commit） |        耗时 |
| ------------------------------------------ | ----------: |
| T0→T1 进程创建 → main JS                   |     1690 ms |
| T1→T2 main JS → app ready                  |      313 ms |
| T2→T3 app ready → loadURL                  |      179 ms |
| T3→T4 loadURL → renderer bundle 开始执行   |     1002 ms |
| T4→T5 bundle → React 首次 commit           |       25 ms |
| **总计（可交互）**                         | **3209 ms** |

同机基线探针：

| 探针                                           |    耗时 |
| ---------------------------------------------- | ------: |
| 裸 Electron 空 main（T0→T1）                   | 1008 ms |
| 空页面 loadURL → did-finish-load（无 preload） |  100 ms |
| 同上 + `out/preload/index.cjs`（689 KB）       |  293 ms |

要点：

- T0→T1 的 1.69 s 里 1.01 s 是 Electron 地板，应用 main bundle 约 680 ms。
- T3→T4 的 1.0 s 里空页面 0.1 s、preload 0.19 s，其余约 0.7 s 是 renderer 初始模块图加载。
- bench 原「ready」= `did-finish-load` ≈ 2.6 s，比 React 首次 commit 早约 0.6 s，不是可交互时间。
- 本 Linux/Xvfb 环境的地板已约 1.3 s，1.8 s 目标无法在这里达成；需要 macOS M1 环境复测再对照。

## 2. 变更

- `desktopWindowChrome`/`desktopHostProcess`/renderer `main.tsx`：bench 模式下输出 T0–T5 阶段行
  （`ESCODE_BENCH_READY_LOG=1`，生产不输出）。
- `bench-escode-app.mjs`：解析 `[bench-startup]` 写入 `summary.json.startupPhases` 与报告。
- 无启动顺序、bootstrap 或渲染入口改动；没有做优化。

## 3. 后续（按收益排序）

1. main bundle 懒加载：削减 T0→T1 应用侧 ~680 ms 的一半以上。
2. preload 瘦身/拆分：当前 689 KB、单次 ~190 ms。
3. renderer 首屏 module graph 收缩：当前 T3→T4 应用侧 ~700 ms。

**2026-10-28 更新**：第 2、3 项已落地（preload 486 KB / ~57 ms；renderer 主 chunk
5.34 MB → 677 KB，设置页/侧栏/代码查看器/工作流弹窗移出首屏），结果见
`docs/reports/rust-u4-startup-optimization-2026-10-28.md`，契约见
`docs/specs/rust-u4-startup-optimization.md`。第 1 项先落地共享包 `sideEffects: false`
树摇标记，安装包 T0→T1 复测待工作树中其他未完成改动落定后进行。

## 4. 验证

- 安装包 C 配置 2 次运行阶段数据一致（上表为最近一次）。
- 基线探针用同一 Electron 二进制、同一 Xvfb。
- `pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed` 通过（见 PR）。
- CI：`desktop e2e matrix` 的 E2E/app-perf 与 rust-runtime-ci 不回退。
