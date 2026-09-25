# Rust 自定义 slash 命令

2026-09-25。用户选定的功能缺口（rust-release-rollback.md「功能缺口范围」）。逐条对齐 TS：

- `adapters/src/commands/{index,roots}.ts`：发现、frontmatter、命名、禁用。
- `contracts/src/commands/index.ts`：模板展开与提示词格式。
- `bootstrap/src/custom-command-{prompt,shell-expansion}.ts`、`custom-commands.ts`：解析入口与 shell 展开。
- `bootstrap/src/slash-command-surface.ts`、`zcode-protocol/slash-commands.ts`：保留名与协议目录。
- `adapters/src/plugins/index.ts`：`resolveCommandRoots` 与 `materializeCommandMetadataRoot`。

## 所有者与流程

````mermaid
sequenceDiagram
  participant H as Host
  participant E as Engine（session owner）
  participant T as ToolPort（命令发现/展开）
  H->>E: sendText "/review a b"（空闲立即执行；忙时入队，出队时才解析）
  E->>E: 内置命令（/compact、/init）先行
  E->>T: resolve_command(text, session)（异步，执行前）
  T->>T: 发现根 → 解析 frontmatter → 模板 → shell 展开（!`cmd` / ```! 块）→ 格式化
  T-->>E: Some(prompt) | None（不是命令或不存在）| Err（shell 失败等 → failed ACK）
  E->>E: admission：模型看到 prompt；userInput 行、标题、#sess 解析仍用原文
````

- 解析时机与 TS `runPromptTurn` 相同：空闲时在 admission 前解析；排队输入在出队执行时解析，而不是在入队时。
- 解析在 Engine 内 await 完成，shell 展开最长 30s（与 TS 超时一致），期间 Engine 不处理其他命令。这是已知限制，
  只有带 shell 语法的命令会触发。
- shell 展开使用自动探测的 shell：admission 阶段没有运行中的 run，无法向 Host 询问终端偏好。
- 展开失败时：
  - 空闲会话以 failed ACK 回复（`fault.command.executionFailed`，附错误文案），不创建轮次，与 Node 相同；
  - 排队输入出队时展开失败，则保留该输入、暂停自动出队，并记录 `lastError`（`custom_command_failed`）。
- 显示与模型分离（TS `displayInput`）：userInput 行文本、自动标题、`#sess_*` 引用都取原始输入；模型消息取展开后的提示词。
  内置 `/init` 同样遵守，修复此前行文本显示展开提示词的差异。

## 规则

- 根目录按优先级依次为：
  - `~/.zcode/commands`、`~/.agents/commands`（user）；
  - cwd 到 git 根之间每一级目录的 `.zcode/commands`、`.agents/commands`（project）；如果不在 git 仓库内，只取 cwd 这一级；
  - 已启用插件的 `commands` 目录与 manifest `commands` 路径（官方插件为 system，其余为 user，source 都是 plugin）；
  - manifest `commands` 为对象时生成的 `<storage>/data/<pluginId>/generated-commands`。
- 递归扫描 `.md` 文件，深度不超过 12，并跟随 symlink。命令名为相对路径，分隔符换成 `:` 后转小写，
  须匹配 `^[a-z0-9][a-z0-9_:-]{0,63}$`。同名命令按优先级先到先赢，最终按名称排序。
- 命中 config `command.<path>.enable=false` 的命令被剔除。
- frontmatter 为扁平 YAML，只认 `description`、`argument-hint`、`allowed-tools`、`model`、`skills`、`disable-noninteractive`。
  description 缺省时取正文首个非空行，去掉 `#`、`-`、`*` 前缀，最长 1024 字符。正文读取上限 100000 字节。
- 保留名不能被自定义命令占用：内置帮助条目的 name 与 alias，另加 `compress`、`plan`。
- 模板展开：
  - `$ARGUMENTS` 替换为全部参数，`$N` 替换为按引号/转义切分后的第 N 个参数；
  - 有参数但模板没有占位符时，在正文后追加 `User arguments:`；
  - 输出头部为 `Run custom command /x.`、`Command source: scope/source.` 与 skills 指令。
- shell 展开：
  - 按出现顺序执行，只替换 stdout（去掉尾部空白）；
  - 环境变量注入 `CLAUDE_/ZCODE_PROJECT_DIR`、会话 id 与插件变量；
  - 用到缺失上下文的变量时直接报错；
  - 非零退出时按 TS 文案报错。
- 协议目录 `slashCommands` 依次为：
  - 内置 `goal`、`compact`、`init`；`workflow` 不列出，因为 Rust 声明动态工作流不支持，与 TS 开关关闭时相同；
  - 仅 App 可用的 `plan`；
  - 自定义命令，排除 `disable-noninteractive` 和保留名，`inputHint` 为 `/name argument-hint`。

## 验收

- `scripts/generate-zcode-cli-rust-custom-commands.mjs` 从 TS 生成：
  - 内置目录与保留名资产；
  - 真实 adapter 与模板函数在固定目录树上的发现与展开语料。
    Rust 须逐条一致。
- App 差分：同一 workspace 下 Node 与 Rust 需要满足以下各项一致：
  - `workspace/readPresentation` 与 `session/read` 的 `slashCommands`；
  - 模型收到的用户消息与 userInput 行，覆盖以下输入：
    - 带位置参数与 shell 展开的项目命令；
    - 子目录命令；
    - user 级 `.agents` 命令；
    - 插件目录命令；
    - manifest 生成的命令；
    - `/init`；
    - 不存在的命令；
  - shell 失败时的 ACK。

  标题由 Node 辅助模型生成，属于已知缺口，不在比对范围内。
