# Desktop 本地 Crash 留档

Desktop 主进程在启动早期配置 crash dump 路径，并在本地归档原始 minidump，方便用户或开发排查
（`desktopCrashCapture.ts`）。远端 crash 上报通道已随遥测功能删除，不再初始化 ARMS crash collector。

本地 `archive` 保留原始文件；`render-process-gone` / `child-process-gone` 仍写入本地日志并触发归档。

## 目录结构

```
<appConfigDir>/crash/
  ├── live/        ← Electron / Crashpad 实时写入目录（`app.setPath("crashDumps")`）
  └── archive/     ← ESCode 额外保留的本地副本
```

`appConfigDir` 默认 `~/.escode/v2/`；用户配置了自定义 `dataBaseDir` 时，路径为 `<dataBaseDir>/.escode/v2/crash/`（依赖 `desktopEarlyDataBaseDirBootstrap` 在 crash 初始化前注入 `dataBaseDir`）。

## 本地归档保留策略

`archive/` 只用于保留原始 minidump，供线上事件不足以定位问题时离线排查；它不参与
Crashpad 采集或 ARMS 上报。为了避免历史 dump 无限占用用户磁盘，本地归档遵循以下规则：

- 最多保留最近 5 个 `.dmp`
- 除最新一个外，归档总大小不得超过 100 MiB
- 最新一个 `.dmp` 始终保留，即使它本身超过 100 MiB
- 删除旧 `.dmp` 时一并删除同名 `.dmp.json` 元数据；若元数据删除暂时失败，后续清理会识别并重试删除不存在同名普通 `.dmp` 的孤立元数据，不处理其他 JSON 文件
- 清理只作用于 `archive/`；`live/` 仍完全由 Electron Crashpad / ARMS 管理

清理在每次归档完成后执行，按 `.dmp` 修改时间从新到旧保留。这样既限制历史文件累积，
又保证最近一次原生崩溃仍有本地原始现场可供分析。若最新单个 dump 本身异常巨大，目录允许
临时超过 100 MiB；这是保留最近崩溃诊断能力的有意例外，而不是严格磁盘配额。

## 启动顺序

1. `desktopEarlyDataBaseDirBootstrap`：同步读取 `setting.json` 的 `dataBaseDir`
2. `appCrashCaptureBootstrap`：配置 `crashDumps`、归档上一轮残留 `.dmp`
3. `appARMSBootstrap`：初始化 ARMS（`collectors.crash: true`）
4. `index.ts`：`app.setPath("userData")` / `sessionData` 等
5. `registerCrashEventMonitor`：renderer / 子进程崩溃后延迟归档（避免 live 目录被 SDK 清理后无本地副本）

## 相关文件

- `packages/desktop/src/main/desktopDataBaseDirBootstrap.ts`
- `packages/desktop/src/main/desktopEarlyDataBaseDirBootstrap.ts`
- `packages/desktop/src/main/appCrashCaptureBootstrap.ts`
- `packages/desktop/src/main/desktopCrashCapture.ts`
- `packages/desktop/src/main/appARMSBootstrap.ts`
- `packages/desktop/src/main/index.ts`
