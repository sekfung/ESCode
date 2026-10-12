# LLM Wiki — Phase 1 源码调研报告（Rust runtime）

> 目标：在 `apps/escode-cli-rust` 中实现 LLM Wiki，由 Rust runtime 直接运行；Desktop 通过既有
> Host→runtime stdio 协议读取知识图谱。TS runtime（`apps/escode-cli`）不改动。
> 基线：`dev` 与 `origin/dev` 同步（`node scripts/check-workspace-freshness.mjs` 通过）。
> 标注「设计」的条目是基于源码的方案判断，不是已存在行为。

## 1. 运行形态

- 安装态 Desktop 默认 runtime 已是 Rust（`apps/escode-cli-rust/README.md`「桌面安装态默认 runtime 已是 Rust」）；
  monorepo 开发态默认仍为 Node，`pnpm dev:desktop:escode-cli-rust` 显式选择 Rust。
- 单一二进制 `escode-cli-rust`（`src/main.rs`）：`app-server --stdio` 为协议入口，其余参数走 `-p` 无头模式（`src/headless`）。
- crate 边界（`docs/specs/rust-cli-architecture.md`、`CONTRACT.md`）：
  `protocol`/`domain`（纯）→ `core-api`（端口）→ `core`（Session actor、Agent loop，不做 IO）；
  `state`/`model`/`tools`/`host` 为 adapter；`app-server`/`tui` 为前端；`src/runtime.rs` 是唯一组合根。
  `scripts/check-escode-cli-rust-boundaries.mjs` 强制：domain 不得用 tokio/reqwest/rusqlite/`std::fs|process|net`；
  core 不得引用 adapter；adapter 不得引用 core；单文件 ≤ 400 行。

## 2. Agent Runtime 关键调用链

```
Host NDJSON → app-server stdio → Engine::request (core/app/engine_requests.rs) → v4/command
  → input admission（core/app/input_admission.rs）→ Session owner 提交 user 消息
  → run.rs 启动 run → agent_loop::run (core/app/agent_loop.rs)
       memory_run::resolve          （项目记忆，经 ToolPort）
       skills::initialize / shell_notice::resolve
       tools.scoped_definitions + adapt_to_model（工具面）
       turn_hooks::before_turn      ← 本轮输入前追加隐藏上下文的既有接入点
       agent_loop_surface::prepare_tool_surface
       loop { ModelPort::complete → tool_dispatch::execute → commit receipt }
```

- **隐藏上下文注入**：`turn_hooks::before_turn` 生成 `{"role":"user","content":<system-reminder>,"_escode_source":"hook_context"}`，
  经 `Event::HookContext { message, before_input: true, committed }` 交给 Session owner；owner 在
  `Engine::hook_context`（`turn_hooks.rs:182`）把消息插到本轮输入前、标记 `history_rewrite` 并 `persist`，
  然后回执；loop 收到回执后 `history.insert(input_start(..))`。消息只进 canonical messages，不产生 UI row。
  → **Wiki 检索注入复用这条 owner 提交路径**（设计：新增 `Event::WikiContext`，由同一处理函数提交）。
- **Token 预算**：`domain::context::estimate`；自动压缩后保留段不含已压缩消息，因此以「上下文中已存在的 wiki 消息」作为去重依据，压缩后自然重置（设计）。
- **工具分派**：`core/app/tool_dispatch.rs` 按名称把 core 原生工具（ListModels、AskUserQuestion、Todo、Cron…）分派到 core 处理函数，其余交给 `ToolPort::execute_scoped`。
  工具定义来自 `ToolPort::scoped_definitions`（`tools/src/tool_schemas.json`、`tool_surface.json`，由
  `scripts/generate-escode-cli-rust-tool-schemas.mjs` 从 TS 契约生成，用于 TS/Rust 对齐）。
  Wiki 工具只存在于 Rust runtime，定义由 Rust domain 提供（与 `ReplyToChannel` 一样不来自 TS builtInTools），在 agent_loop 中按 Wiki 状态追加。
- **权限**：`core/app/permission_flow.rs` 用 `ToolPort::permission_capability`（`tools/src/tool_capability.rs`，生成表）
  与 `domain::permission::check`；`always_ask` 在 yolo 下仍询问（`domain/src/permission.rs:228`）。
  Wiki 工具能力由 domain 表提供，`WikiManage` 声明 `alwaysAsk`。
- **RPC**：`Engine::request` 按方法名分派（如 `v4/usage/stats`）；Desktop `packages/services/src/escode-agent/escodeAgentService.ts`
  用 `client.request(V4_METHODS.x, params, zodSchema)` 调用，UI 经 `services.escodeAgentService` 访问（例：`packages/ui/src/hooks/useSessionSubagents.ts`）。
  method 常量与 schema 在 `packages/shared/src/escode-protocol-v4/transport.ts`。

## 3. 项目身份、配置与持久化

- workspace：`runtime::resolve_workspace`（`src/runtime.rs`）；identity = `escode_cli_host::workspace_identity(ESCODE_WORKSPACE_IDENTITY, requested_cwd)`，
  使用 Host 原路径而非 realpath（「不把 macOS /var → /private/var 改写成新工作区」）。
- 数据目录：`--data-dir` / `ESCODE_CLI_RUST_DATA_DIR`，默认 `~/.escode/rust`。
- 用户/项目配置：`tools/src/extension_config.rs`（`~/.escode/config.json` + 项目 `.escode/config.json` 分层合并，4 MiB 上限），
  `storage()` 推导 `<storage.dir>/cli/...`；项目记忆在 `<storage>/cli/memories/projects/<slug-hash>/memory`（`tools/src/project_memory.rs`）。
- 现有项目身份只用路径哈希；没有 Git 远程身份、worktree 归并。
- YAML：domain 已有 `saphyr` 解析（`saved_workflow.rs::to_json`）与 `yaml_emit::stringify`。
- 已锁定依赖：`zip 8.6`（deflate-flate2）、`flate2`、`sha2`、`globset`、`regex`、`chrono`、`saphyr`、`uuid`、`tempfile`。

## 4. 可复用能力与差距

| 能力 | 位置 | 用法 |
| --- | --- | --- |
| owner 提交隐藏上下文 | `turn_hooks.rs` `hook_context` | Wiki 注入 |
| core 原生工具分派 | `tool_dispatch.rs` | Wiki 工具 |
| 权限能力 + alwaysAsk | `permission_flow.rs`、`domain::permission` | 治理工具确认 |
| 配置分层 | `tools/src/extension_config.rs` | 设计：Wiki 适配器读取同一份 `wiki` 段（不新增配置文件） |
| 文件日志 | `escode_cli_host::file_log` | 生命周期与诊断 |
| 原子目录/文件写 | `tools/src/file_atomic.rs`、`atomic_dir.rs` | 参考实现；Wiki crate 自带同等实现以免依赖 tools |
| ZIP | `zip` crate | DOCX/XLSX/PPTX |
| YAML | saphyr / yaml_emit | frontmatter、YAML 导入 |

差距（需新增）：XML 解析（OOXML）→ `quick-xml`；文本型 PDF → `lopdf`；Markdown/CSV/RST 按行自研（定位精确、无依赖）。
仓库没有关键词索引/检索、知识条目 schema、导入管道、知识图谱。

## 5. 方案：新增与修改

| 位置 | 内容 |
| --- | --- |
| `crates/domain/src/wiki/*`（纯） | 条目 schema 与校验、frontmatter、ID、分词、倒排索引、排序与预算、工作流识别、抽取（IR→候选）、冲突、lint、reminder、图谱投影、工具定义与能力 |
| `crates/wiki`（新 adapter crate `escode-cli-wiki`） | 存储（原子写、目录锁）、项目身份（.git/worktree/remote）、解析器（Markdown/TXT/CSV/TSV/JSON/YAML/XML/DOCX/XLSX/PPTX/PDF/RST/DOC/XLS 转换器）、导入/同步、治理、审计、`WikiPort` 实现 |
| `crates/core-api` | `WikiPort` trait 与数据类型；`RuntimePorts.wiki` |
| `crates/core` | `wiki_context.rs`（每轮检索注入）、`wiki_tool.rs`（4 个工具）、`v4/wiki/*` RPC、权限能力接入 |
| `src/runtime.rs`、`src/main.rs` | 组合 Wiki 端口；`escode-cli-rust wiki …` 运维子命令 |
| `scripts/check-escode-cli-rust-boundaries.mjs` | 把 `wiki` 列为 adapter crate |
| `packages/shared` | `v4/wiki/*` method 与 zod schema（严格运行时校验） |
| `packages/services` | `escodeAgentService.wiki*` 请求 |
| `packages/ui` | 知识图谱页面（桌面 + 手机 Web） |

## 6. 测试入口

- `pnpm check:escode-cli-rust`（边界 + fmt + clippy -D warnings）、`pnpm test:escode-cli-rust`（`scripts/test-escode-cli-rust.mjs`）、
  `cargo test -p <crate>`。集成测试启动真实二进制与本地 HTTP 模型 fixture（README「构建与验证」）。
- 前端：`pnpm typecheck`、`pnpm lint`、vitest（`packages/ui`、`packages/shared`）、Desktop E2E（`pnpm test:e2e`，需 Electron）。

## 7. 风险

- 每轮检索必须本地、确定性、不调用模型；索引按版本缓存。
- 工具面仅在 Wiki 有内容或显式配置时变化，避免影响既有会话的 prompt cache 与对齐测试。
- 文档内容不可信：reminder 声明「数据非指令」，转义包裹标签；不执行宏/脚本，XML 不展开实体。
- 新增 Rust 依赖（`quick-xml`、`lopdf`）需检查许可证（均 MIT）并更新 THIRD-PARTY 声明。
- 环境：容器 Node v22.22（要求 24.14），Electron 二进制下载失败 → Desktop E2E 无法在此运行。
