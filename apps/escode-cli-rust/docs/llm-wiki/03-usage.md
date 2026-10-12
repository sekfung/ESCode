# LLM Wiki 使用指南（Rust runtime）

LLM Wiki 是 ESCode 的持久化知识库：把团队规范、架构决策、接口契约、业务规则等整理成可追溯、可审核的知识条目，
Agent 在编码和 Code Review 时自动检索相关条目并注入上下文。规则与数据结构见 [01-spec.md](01-spec.md)，
实现与验证结果见 [02-delivery-report.md](02-delivery-report.md)。

> 只有 Rust runtime（`escode-cli-rust`）实现 LLM Wiki。桌面安装包默认使用 Rust runtime；
> 如果设置页「知识库」显示「当前 runtime 不支持」，说明当前 workspace 跑的是 Node runtime（开发态或 `ESCODE_AGENT_SERVER_RUNTIME=node`）。

## 1. 两层作用域

| 作用域 | 存储位置 | 谁能看到 | 版本管理 |
| --- | --- | --- | --- |
| Global Wiki | `<storage>/wiki/` | 所有项目 | 否（用户本机） |
| Project Wiki | `<项目根>/.escode/wiki/` | 只有这个项目 | 是，随仓库提交 |

- **Global 根目录**：`<storage>` 依次取 `ESCODE_STORAGE_DIR`、用户层 `storage.dir`，都没有时用 `~/.escode`，所以默认路径是 `~/.escode/wiki/`。用户层 `wiki.root` 可直接指定其他位置。
- **项目根**：最近的 Git 工作树根；无 Git 时为打开的 workspace 目录。Project Wiki 跟着用户打开的项目走。

Project Wiki 的目录内容：

```
.escode/wiki/
  entries/<id>.md    提交：知识条目（含来源引用、审核历史）
  project.json       提交：固定的项目 ID
  .gitignore         提交：忽略下面的本机数据
  sources/ ir/ index.json audit.jsonl .lock/   不提交（本机路径、派生缓存、审计、锁）
```

- 目录只在第一次写入时创建（导入、提交候选、审核等）。只是打开项目、检索或查看图谱不会往仓库里写文件。
- 队友拉取仓库后直接可用：条目随仓库到达，`project.json` 保证项目 ID 一致。在新克隆里再次导入同一份文档，会命中已有条目，不会产生重复。
- 批准、弃用等操作会修改 `entries/` 下的文件，应像普通代码一样提交并经过评审。仓库里已提交的 `approved` 条目即视为团队批准的规范。

项目身份（用于生成首个项目 ID 和展示，写入 `project.json` 后以文件为准）按以下优先级确定：
1. Git 远端 URL（规范化后，同一仓库的不同克隆、工作树共享知识）。
2. 无远端时用 Git 本地根目录。
3. 远程 workspace 使用 `workspaceIdentity`。
4. 都没有时用规范化路径。

`wiki status` 会显示当前身份、依据和两个存储的实际路径。

## 2. 导入文档

```bash
# 导入为候选（默认）：不会自动成为规范
escode-cli-rust wiki ingest docs/ handbook.docx rules.xlsx

# 导入并直接批准（高影响操作：交互终端需确认，脚本里需 --yes）
escode-cli-rust wiki ingest docs/standards --approve

# 导入到全局 Wiki
escode-cli-rust wiki ingest ~/team/coding-standards.md --scope global
```

- 支持格式：Markdown、TXT、CSV/TSV、JSON、YAML、XML、RST、DOCX、XLSX、PPTX、文本型 PDF。
  - DOC/XLS 需要在用户配置中开启 Office 转换器，见下文配置。
  - 扫描型 PDF 会被识别并报告为「需要 OCR」，当前未内置 OCR。
- 每个条目记录来源文档、具体位置（标题、行号、页码、单元格、记录、JSON/YAML 路径、幻灯片）和原文摘录。
- 重复导入是幂等的。未变化的文件直接复用解析结果；修改后只更新受影响的条目。
  - 如果某条已批准知识的来源发生变化，系统会生成一条修订候选，交由你审核，不会静默覆盖。
- 删除或移动源文件不会删除知识：
  - 运行 `wiki sync` 检测变化、移动和缺失。
  - 来源缺失的条目会被标记为 `broken-source` 并提示复核。

## 3. 检索与 Agent 使用

- **自动注入**：主会话每轮有用户输入时，按任务类型（coding、debugging、testing、refactoring、review、general）检索。
  - 通过相关性门槛的条目以隐藏上下文的形式插在本轮输入之前。
  - 注入量受 `maxResults` 与 `maxInjectedTokens` 限制。
  - 同一条目的同一版本在一个会话内只注入一次；压缩上下文后允许重新注入。
- **工具**：模型可以调用以下四个工具。
  - `WikiSearch` / `WikiRead`：检索并读取全文与来源。
  - `WikiPropose`：提交候选知识，永远不会自动生效。
  - `WikiManage`：每次调用都需要你确认，yolo 模式也不例外。
- **Code Review**：识别为 review 的任务会附加评审协议，要求 finding 基于实际 diff，引用 `[wiki:<id>]` 及其来源。
  - `needs-review` 状态的条目不能作为违规依据。
- **安全**：知识内容一律是「参考数据」，不会覆盖系统或安全约束；文档中的指令性文本会被转义包裹。

命令行检索：

```bash
escode-cli-rust wiki search "API error envelope"
escode-cli-rust wiki search retry policy --workflow review --include-proposals
escode-cli-rust wiki read api-error-envelope
```

## 4. 审核流程

1. **产生候选**：来源可以是导入（默认）、Agent 调用 `WikiPropose`，或 `wiki propose --title … --kind … --body … --rationale …`。
2. **查看**：
   - Desktop：设置 → 知识库 → 点击图谱节点，查看正文、来源定位、冲突和审核历史。
   - 命令行：`wiki read <id>`、`wiki conflicts`。
3. **决定**：
   - `approve`：成为规范，并按 kind 授予权威等级。
   - `reject`：永不注入。
   - `deprecate`：停止注入，可用 `--superseded-by` 指定替代条目。
   - Desktop 上每个操作都会弹出确认框。
4. **提升到全局**：`wiki promote <id>` 把项目条目复制到 Global Wiki，作为新的候选，仍需单独批准。

所有审核操作都写入条目的 `review` 历史和作用域的 `audit.jsonl`，记录发起方是 cli、desktop 还是 `agent:<session>`。

## 5. 维护与诊断

| 命令 | 作用 |
| --- | --- |
| `wiki status` | 项目身份、存储位置、各作用域条目与来源统计、索引版本、配置问题 |
| `wiki lint` | schema、重复 id、断链、长期未审核的候选等问题 |
| `wiki validate` | 检查来源文件是否被修改、移动或删除（只报告） |
| `wiki sync` | 处理来源变化（重新解析或标记失效） |
| `wiki conflicts` | 规范矛盾、Global 与 Project 规则冲突、重复、待处理修订、失效来源、代码路径或版本不匹配 |
| `wiki rebuild-index` | 从条目文件重建关键词索引（索引损坏时也会自动重建） |
| `wiki audit --limit 50` | 最近的审计记录（检索只记录查询哈希，不记录原文） |
| `wiki graph` | 图谱节点与边，与 Desktop 图谱页使用同一数据 |

所有命令都支持 `--cwd <workspace>` 和 `--json`。

## 6. Desktop 知识图谱

位置：设置 → Agent 能力 →「知识库」。桌面与手机 Web 使用同一页面；手机宽度下图谱与详情改为上下排列。

- **节点颜色**表示类型（规范与规则、架构与契约、指南与模式、参考、反模式、候选类）。
  - 虚线描边：候选；警示描边：待复核；淡化：已弃用、已取代或已拒绝。
  - 方块节点：来源文档。
- **边**有四种：related、supersedes（虚线）、source、conflict（红色虚线）。
- 可按作用域、状态、类型筛选，也可以隐藏来源节点；超过 500 个节点时会提示缩小筛选范围。
- 选中节点后会高亮它的一跳邻居。详情面板对候选提供「批准 / 拒绝」，对已批准条目提供「弃用」。

## 7. 配置

在 `~/.escode/cli/config.json`（用户层）或项目的 `escode.json` / `.escode/config.json`（项目层）中写 `wiki` 段，完整字段见 [01-spec.md §12](01-spec.md)。

```jsonc
{
  "wiki": {
    "enabled": true,
    "autoInject": true,
    "retrieval": { "maxResults": 5, "maxInjectedTokens": 1500, "minScore": 0.3 },
    "formats": { "pdf": true, "pptx": false },
    "converters": { "office": { "enabled": true, "command": "soffice" } }
  }
}
```

- 项目层不能修改以下几项，写了会被忽略并在 `wiki status` 中报告：
  - `root`、`converters`、`ocr`。
  - 原因：防止仓库内配置重定向 Global Wiki 存储或执行外部命令。
- `projectStorage` 已移除：Project Wiki 固定在项目内的 `.escode/wiki/`。
- `enabled: false` 时不注入、不提供工具；Agent 其余功能不受影响。
