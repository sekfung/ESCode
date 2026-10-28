# Rust 阶段 4：冷启动 preload / renderer 优化结果（2026-10-28）

父计划：`docs/specs/rust-unified-host.md` §阶段 4「启动/退出路径优化」。
契约：`docs/specs/rust-u4-startup-optimization.md`。基线：`docs/reports/rust-u4-startup-phases-2026-10-28.md`。

## 1. 结论

preload 与 renderer 首屏模块图两项已落地：

| 指标                               | 优化前         | 优化后         |
| ---------------------------------- | -------------- | -------------- |
| `out/preload/index.cjs`            | 689 KB         | 486 KB         |
| `pluginSandbox.cjs`                | 664 KB         | 407 B          |
| `resourceManager.cjs`              | 672 KB         | 8.6 KB         |
| 裸 Electron preload 加载探针       | 293 ms         | 133 ms         |
| T3→T4 中 preload 段（安装包）      | ~190 ms        | ~57 ms         |
| renderer 主 chunk（`styles-*.js`） | 5,335,922 B    | 676,992 B      |
| renderer 首屏静态图                | 303 / 8.45 MiB | 272 / 6.50 MiB |
| main 首屏静态图                    | 1.67 MiB       | 1.08 MiB       |

main 侧另做了一步 zod 语言包裁剪：zod v4 的 `core/index`/`classic/external`/`mini/external`
以命名空间 re-export 全部语言包，调用方不用 `z.locales` 也会把 64 个语言文件（静态图约
267 KB）打进 main。应用默认 en、未调用 `z.config(z.locales.*)`，构建期替换为只导出 `en`
的 shim（`src/main/zodLocalesEnOnly.ts` + `esbuildPlugins` 解析插件）。main 静态图
617 KB chunk 降到 359 KB。

安装包复测（同一 Linux/Xvfb，C 配置）：

| 阶段（T0→T5）            | 基线    | 复测（zod 裁剪前 → 后）                                                                                |
| ------------------------ | ------- | ------------------------------------------------------------------------------------------------------ |
| T0→T1 进程创建 → main JS | 1690 ms | 1348 / 1392 / 1414 / 1414 / 1475 / 1495 ms（裁剪前会话中位 1414）；1425–1583 ms（裁剪后会话中位 1482） |
| 同机 Electron 地板       | 1008 ms | 958–1043 ms（中位 1019）                                                                               |
| 应用 main 侧             | ~680 ms | 基线会话 ~395 ms（-41%，最好 1348 → -51%）；裁剪后会话 ~462 ms                                         |
| T3→T4 loadURL → bundle   | 1002 ms | 548–681 ms（中位 ~597，-40%）                                                                          |
| T0→T5 可交互             | 3209 ms | 2446–2743 ms（中位 ~2600，-19%）                                                                       |

同窗口 A/A：1.34 MiB 静态图控制组中位 1501 ms，1.08 MiB 裁剪组中位 1482 ms，差 19 ms，
落在运行噪声内。结论：**main 侧静态图削减 35% 已落地，但 T0→T1 未稳定超过 50%**；
最好样本达到 -51%。T1 中段看起来由 asar/Electron 固定 I/O 与主进程顶层初始化主导，
继续压缩模块体积收益有限，下一步应测顶层初始化分项而不是继续拆包。

## 2. 变更

### preload

- `packages/shared/package.json` 新增子路径导出：`./channels`、`./database-startup`、
  `./process-names`。
- 六个 preload 入口改为深子路径导入，不再经过 `@escode/shared` barrel：
  `index.ts`、`pluginSandbox/index.ts`、`pluginSandbox/hostBridge.ts`、
  `cuaPermissionPanel.ts`、`embeddedBrowserJavaScriptDialog.ts`、`embeddedBrowserWheel.ts`、
  `resourceManager.ts`。
- 删除 `index.ts` 中未使用的 `shouldEnableE2ETestBridge` 导入。

### renderer

- `CodeBlock` 改为 `lazy(() => import(code-viewer))`，`@pierre/diffs`/`CodeViewer` 不再进入
  首屏；纯文本 fallback 保持代码内容可见。
- `Root` / `WorkspaceSettingsLayer`：设置页改为按需加载，仅在设置 tab 激活时拉取
  `SettingsPage` chunk。
- `WorkspaceShellLayout`：`AnimatedSidePanePanel`、Automations / PluginStore / Knowledge
  主视图改为按需加载；`AUTOMATIONS_TOAST_ANCHOR_ID` 拆到独立模块，避免为常量引入整份
  AutomationsSection。
- `WorkflowRunSaveControls`：`SavedWorkflowLaunchDialog` 仅在有 launchEntry 时加载。
- `shikiHighlighter`：静态 `import { bundledLanguages } from "shiki"` 改为读取生成的
  `shikiLanguageRegistry.generated.ts`，真实高亮仍走 `import("shiki")`；
  新增 `scripts/generate-ui-shiki-language-registry.mjs`（含 `--check`）与漂移测试。

### 树摇与 main 懒加载

- `packages/shared`、`packages/services`、`packages/provider`、`packages/provider-node`、
  `packages/rpc`、`packages/client` 增加 `"sideEffects": false`，让 barrel 导入按实际使用裁剪。
  这些包当前均无顶层注册/全局写入；若后续新增，必须在评审中处理。
- `desktopWslTargetResolver` 的 `WSLBackend`、`openInEditor` 的 `listWSLDistros` 改为
  `await import()`，WSL/remote 子系统不进入 main 首屏图。
- `tsup.config.ts` 的 main 构建增加 `esbuildPlugins: [zodLocalesEnOnlyPlugin()]`：把 zod
  `locales/index.js` 解析到 `src/main/zodLocalesEnOnly.ts`（只导出 `en`），裁剪约 267 KB。

## 3. 验证

- `pnpm typecheck`：通过（全量 project references）。
- `packages/ui` 定向 vitest：`workflowSaveControls`（22）、`rootWorkspaceSettingsLayerServices`、
  `animatedSidePanePanelLayout`、`sidePaneTerminalWindowsIme`、`codeViewerComponent`、
  `previewPaneFileContent`、`shikiHighlighter`、`firstSegmentProbe` 共 83 项通过。
- `pnpm exec tsc -p packages/ui/tsconfig.json --noEmit`、E2E 配置中新增 perf 探针无类型错误。
- `node scripts/audit-renderer-initial-graph.mjs --check`：0 个禁用 chunk。
- `desktopWslTargetResolver`（2）、`openInEditor`（18）在改为动态导入后通过。
- `cargo test --manifest-path apps/escode-host/Cargo.toml --workspace`：550 项通过；
  `cargo clippy --workspace --all-targets -- -D warnings` 通过（含 skill 目录缓存改动）。
- 安装包阶段探针：C 配置 5 次、Electron 地板 5 次，见 §1。
- `pnpm lint`：0 error（仓库既有 2144 条 warning 不在本切片范围）。

## 4. 未完成

- main 侧静态图已从 1.67 MiB 降到 1.08 MiB（-35%），但 T0→T1 未稳定达到 >50%：
  同窗口 A/A 显示再削 250 KB 只换来约 19 ms；T1 中段由 asar/Electron I/O 与顶层初始化
  主导。下一步应先给 main 顶层初始化（`BrowserGuestManager`、`createSettingService`、
  autoUpdater 等）加分段计时，再决定是否把设置服务/浏览器 guest 管理改为延迟初始化，
  而不是继续按体积拆包。
- `pnpm --filter @escode/desktop build` 全链路（含 AppImage）未跑；本轮用
  `build:no-runtime-assets` + `electron-builder --linux dir` 复测。
- 首段冷态优化（301 / 410 ms）只补了分阶段探针，尚未根据分段数据调整投递/渲染节奏。
  本地尝试跑 `app-perf-first-segment.perf.ts` 时，E2E onPrepare 需要构建
  `apps/escode-cli`，该包当前正被并行改动（`@escode/bootstrap` tsc 报
  `TurnExecutionModelFallbackReason` / `archivedAt` 类型错误），因此没有拿到分段数据；
  等该改动落定后由 `desktop e2e matrix` 的 app-perf 作业产出。
- zod 语言包裁剪的边界：应用未使用 `z.locales`，保留 en；若未来需要 zod 本地化错误文案，
  必须在引入处显式 import 对应 locale，不能依赖被替换的索引模块。

## 5. 复测命令

```
pnpm --filter @escode/desktop run build:no-runtime-assets
node scripts/audit-renderer-initial-graph.mjs --check
pnpm --filter @escode/desktop exec electron-builder --linux dir --config electron-builder.config.js
node scripts/bench-escode-app.mjs \
  --installed-app packages/desktop/dist/linux-unpacked/escode-preview \
  --installed-glm packages/desktop/dist/linux-unpacked/resources/glm \
  --configs C --reps 3
```
