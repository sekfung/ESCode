# Rust 斜杠命令与内置提示词展开（WP4）

2026-09-24。TS 在提交前把内置提示词命令展开成普通用户提示词（`bootstrap/builtin-prompt-command.ts`），
并用 `bootstrap/zcode-protocol/slash-commands.ts` 给 App 下发命令目录。Rust 之前两者都没有。

## 本次实现

- `/init [args]`：展开为 TS 逐字一致的提示词（目标文件 `AGENTS.md`、隐藏候选 `.zcode/AGENTS.md` 与
  `.agents/AGENTS.md`、附加指示、Process 与建议内容清单）。工作目录取会话 workspace 路径。
  展开发生在 admission 之前，与 `/compact` 同一位次，其余 admission 语义不变。
- `/workflow`：Rust 无动态工作流，恒按关闭处理（TS 在 `dynamicWorkflowEnabled: false` 时同样不展开）。

## 命令目录策略

`workspace_config` 的 `slashCommands` 只列出 Rust 真正实现的命令：当前为 `compact`。
`goal`、`init`、`plan`、`workflow` 需要各自的功能先落地（goal 续跑语义、plan 审批交互、工作流引擎），
在此之前不下发，避免 App 展示无法执行的入口——与 TS 在功能开关关闭时的做法一致。

自定义命令（`listZCodeCustomCommands` 的文件发现、保留名过滤、`/init` 之外的展开）仍未实现，
需要与 Skill 发现同源的插件/用户命令根目录扫描；列为后续工作。

## 验收

1. 差分：`scripts/generate-zcode-cli-rust-init-prompt-corpus.mjs` 以 TS 为 oracle 覆盖 20 条输入
   （参数形式、大小写、多行、中文、前后空白、非 init 命令、空输入），Rust 逐条比对展开结果或 null。
2. App 集成（待可构建环境）：`/init` 输入后模型收到的是展开提示词而非字面量；目录中不出现未实现命令。
