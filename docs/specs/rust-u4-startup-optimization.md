# Rust 阶段 4：冷启动三项优化（rust-unified-host.md 超越项 3 补充）

父计划：`docs/specs/rust-unified-host.md` §阶段 4「启动/退出路径优化」。
前置：`docs/specs/rust-u4-startup-phases.md`（T0–T5 口径与基线）。

## 1. 问题与目标

阶段拆分显示安装包冷启动 T0→T5 为 3209 ms，其中应用侧可优化项：

| 项                          | 基线            | 目标                              | 结果（2026-10-28）                            |
| --------------------------- | --------------- | --------------------------------- | --------------------------------------------- |
| preload 单次加载            | 689 KB，~190 ms | 拆到按用途最小入口，加载 ≤60 ms   | 486 KB / ~57 ms，达标                         |
| renderer 初始模块图         | ~700 ms         | 首屏不需要的页面/面板不进入静态图 | 8.45 → 6.50 MiB，禁用 chunk 0 个              |
| main bundle（T0→T1 应用侧） | ~680 ms         | 懒加载/树摇后削减 ≥50%            | 静态图 1.67 → 1.08 MiB（-35%）；T1 未稳定达标 |

main 侧已做共享包 `sideEffects: false` 树摇、WSL/remote 探测按需加载，以及 zod 语言包裁剪
（64 个 locale、静态图约 267 KB → 仅 en）。同窗口 A/A 显示再削 250 KB 只换来约 19 ms；
T1 中段由 asar/Electron I/O 与顶层初始化主导，继续按体积拆包收益有限。下一步应给 main
顶层初始化加分段计时，再决定是否延迟初始化设置服务与 `browserGuestManager`。

## 2. 范围与非目标

范围内：

- preload 六个入口改为从 `@escode/shared` 深子路径导入，不再经过 barrel；新增
  `./channels`、`./database-startup`、`./process-names` 子路径导出。
- renderer 懒加载边界：`CodeViewer`、`SettingsPage`（含 workspace 设置层）、
  `AnimatedSidePanePanel`、Automations/PluginStore/Knowledge 主视图、
  `SavedWorkflowLaunchDialog`。
- `@escode/shared`、`@escode/services`、`@escode/provider*`、`@escode/rpc`、`@escode/client`
  标记 `sideEffects: false`，让 main/renderer 的 barrel 导入可被树摇。
- main 的 WSL backend 与 WSL distro 探测改为按需加载，不进入 T0→T1 静态图。
- main 构建把 zod 语言包索引替换为只导出 `en` 的 shim（应用未使用 `z.locales`）。
- 对应 spec/测试更新与安装包阶段复测。

非目标：

- 不改变首屏布局、交互顺序或设置页/侧栏的挂载语义。
- 不调整 T0–T5 定义，不改 bench 判定阈值。
- 不把首个会话渲染所需的模块（`SessionPane`、composer、lexical、i18n、终端面板）移出首屏。
- 不做 renderer 资源分包策略重写（manualChunks/rolldown 配置）。

## 3. 所有权与数据流

```
preload/index.cjs ──只依赖需要的 shared 子路径──▶ channels / database-startup / process-names

renderer main.tsx ──▶ App ──▶ WorkspaceShellLayout
                              ├─ 首屏：SessionPane / composer / sidebar
                              ├─ lazy: AnimatedSidePanePanel（默认收起）
                              ├─ lazy: Automations/PluginStore/Knowledge（tab 打开时）
                              └─ lazy: SavedWorkflowLaunchDialog（点击「再次运行」）
       Root ──▶ lazy: SettingsPage / WorkspaceSettingsLayer（设置 tab 激活时）
       markdown CodeBlock ──▶ lazy: CodeViewer（@pierre/diffs）
```

- 懒加载边界由 React `lazy` + `Suspense` 持有；fallback 不得改变布局占位（侧栏为空、
  设置页复用启动 loading、代码块用纯文本占位）。
- 组件首次打开时加载 chunk；已加载后与静态导入行为一致。
- 静态图约束以构建产物验证，不以源码 import 文本为准。

## 4. 契约

- **preload 入口预算**：`out/preload/index.cjs` ≤500 KB；`pluginSandbox`/`resourceManager`
  等入口不因 barrel 导入回涨；禁止从 `@escode/shared` 根入口导入运行时值。
- **renderer 静态图**：`main.tsx` 的静态 import 闭包不得包含设置页、侧栏面板、
  Automations/PluginStore/Knowledge 主视图、`SavedWorkflowLaunchDialog`、`@pierre/diffs`。
- **懒加载行为**：首次打开对应 UI 时必须渲染完整内容；测试需等待异步 chunk（`findBy*`）。
- **树摇**：标记为 `sideEffects: false` 的包（shared/services/provider/provider-node/rpc/client）
  不得新增顶层副作用；一旦出现（注册、全局写入），必须移除该标记或在包内显式声明副作用文件。
- **可回退**：每个懒加载边界可单独回退为静态导入而不影响其他边界。

## 5. 验收

| 用例            | 断言                                                                       |
| --------------- | -------------------------------------------------------------------------- |
| preload 构建    | 三个入口体积在预算内；bare Electron preload 加载探针 ≤60 ms                |
| renderer 静态图 | 从 `index-*.js` 递归静态 import，设置/侧栏/工作流弹窗 chunk 不在闭包内     |
| 首屏交互        | 首条消息、代码块、设置页、侧栏、再次运行弹窗均可用                         |
| 单测            | `codeViewerComponent`、`workflowSaveControls`、settings/side-pane 相关通过 |
| 安装包阶段      | `bench-escode-app` 输出 T0–T5；preload 与 renderer 阶段相对基线下降        |
| 无回归          | E2E/app-perf 与 rust-runtime-ci 不回退                                     |

## 6. 已知差异与风险

| 项                    | 说明                                                                                                                      |
| --------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| 首次打开有 chunk 延迟 | chunk 在 asar 内，通常 <30 ms；UI 用 fallback 占位，不阻塞首屏                                                            |
| 测试需要异步等待      | 懒加载边界的测试必须用 `findBy*`/`waitFor`，同步 `getBy*` 会失败                                                          |
| SSR 测试              | 直接 SSR 渲染懒加载组件会得到 fallback；测试应渲染被懒加载模块本身                                                        |
| sideEffects false     | 若后续在 shared 顶层加入注册/全局写入，树摇会改变行为，必须在评审中拦截                                                   |
| zod 语言包裁剪        | main 的 `locales/index.js` 被替换为 en-only shim；新增 `z.locales.xx` 用法时必须显式 import 对应 locale，不能依赖索引模块 |
| T1 体积不敏感         | 1.34 → 1.08 MiB 的同窗口 A/A 只差 ~19 ms；继续优化前应先测顶层初始化，而非按体积拆包                                      |
