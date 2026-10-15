# Rust Host：用户配置目录类服务（U2）

> 状态：持续追加 · 2026-10-15。这些服务都是「Host 读写用户家目录下某个约定位置」的薄 FS 服务，没有跨进程状态，
> 差分以 TS 真实服务为 oracle（`HOME` 指向临时目录）。每个服务一节：范围 / 事实源 / 登记差异 / 验收。

## 1. output-style（7 个成员）

TS 真相源：`packages/services/src/output-style/outputStyleService.ts`。

- **事实源**：`<home>/.claude/output-styles/*.md`（自定义风格）与 `<home>/.claude/settings.json` 的 `outputStyle` 键（当前激活）。`home` = Host 的家目录
  （TS：`HOME || USERPROFILE || os.homedir()`，Main 经 `rpcListen.home` 提交同一值）。
- `listStyles()`：三个内置风格（`default/explanatory/learning`，`enabled` = 是否为激活项）+ 自定义风格（`id = custom-<文件名去第一个 .md>`，`name/description`
  取自 `name:` / `description:` 行，缺 `name` 用文件名，再缺用 `Custom Style`；按 `name` 的 `localeCompare` 排序；`filePath` 是文件绝对路径）。
- `addStyle({config})`：文件名 = `name.toLowerCase()` 把连续空白换成 `-`，内容 = `name: …\ndescription: …\n\n<content>`；`updateStyle/deleteStyle` 去掉 `custom-` 前缀定位文件，
  `deleteStyle` 缺文件不报错。
- `getActiveStyle()` → `{styleId: settings.outputStyle ?? null}`；`setActiveStyle({styleId})` 读改写 `settings.json`（缺目录创建；文件损坏按空对象处理并**覆盖**——TS 同）。
- `getUserStylesDirectory()` → `{path}`（顺带创建目录）。

登记差异：

1. `settings.json` 写回时键按字典序（`serde_json` Map），TS 保持原键序；两者互读语义相同，内容缩进同为 2 空格、无结尾换行。
2. 控制台错误日志（`console.error`）不复刻，读失败按 TS 返回 null / 跳过该文件。

验收：`rustHostUserConfig.test.ts` 对真实 TS `createOutputStyleService`（`process.env.HOME` 指向临时目录）与 Rust Host 执行同一操作序列，逐步比较返回值与落盘语义。
