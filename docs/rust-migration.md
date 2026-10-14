# ESCode CLI Rust

这是将 ESCode CLI 从 TypeScript 迁移到 Rust 的工作仓库。Rust 实现位于 [`apps/escode-cli-rust/`](apps/escode-cli-rust/)，以 Cargo workspace 组织协议、Session 核心、模型适配、工具执行、App Server 和 TUI。**自 2026-10-03 起，安装包内的桌面 App 默认使用 Rust runtime**（显式 `ESCODE_AGENT_SERVER_RUNTIME=node` 可回退，见 [`docs/specs/rust-release-rollback.md`](docs/specs/rust-release-rollback.md)）；monorepo 开发态默认保持 Node，用 `pnpm dev:desktop:escode-cli-rust` 显式选择 Rust。

## 核心目标

本仓库的核心任务是把现有 Node.js/TypeScript escode CLI runtime 迁移到 Rust，在保持 App stdio/V4 协议、Session 状态、模型请求、工具执行、持久化和桌面接入语义的前提下，逐步用 Rust runtime 替换 Node.js runtime。功能对齐、性能、数据迁移、跨平台和发布回退五道门槛已通过并完成默认切换。后续桌面 Host 统一化（Rust Host + 进程内 runtime）的路线见 [`docs/specs/rust-unified-host.md`](docs/specs/rust-unified-host.md)。

当前迁移已经覆盖 Rust App Server 的主要使用链路：

- [x] Session、SQLite、队列、冷恢复、compact 和 workspace identity 隔离
- [x] OpenAI Chat Completions、OpenAI Responses、Anthropic Messages
- [x] Provider Registry、模型选择、账号 overlay、yolo 能力协商
- [x] Read/Write/Edit/Glob/Grep/Bash、后台任务、AskUserQuestion、Todo
- [x] MCP（含 OAuth）、Skill、子代理、Goal、retry/edit、fork 和文件回退核心链路
- [x] 文本、图片、PDF 基础附件链路
- [x] 权限模式（yolo/build/edit/guarded）、Workflow/Cron/OffPeak、浏览器工具（node_repl）、本地 TTFT 遥测
- [ ] 剩余长尾（插件 UI 面板、话题附件、记忆 Dream/Semantic Recall、媒体预览、provider hosted tools 等）：权威清单见 [`docs/specs/rust-parity-remaining.md`](docs/specs/rust-parity-remaining.md) 与 [`docs/specs/rust-v3.15.1-drift.md`](docs/specs/rust-v3.15.1-drift.md)

同一台 Apple M2 Pro 上使用本地 SSE fixture、8 回合单 Session、5 次交错测量的结果如下。Node.js 使用当前 CLI bundle，Rust 使用 release 二进制：

| 指标             |   Node.js |     Rust |
| ---------------- | --------: | -------: |
| 空闲 RSS         | 408.8 MiB | 11.2 MiB |
| 负载采样峰值 RSS | 522.2 MiB | 23.4 MiB |
| 累计 CPU time    |    0.78 s |   0.10 s |
| 8 回合耗时       |   1.289 s |  0.147 s |

这组数据只代表当前本地 fixture 和构建形态；真实供应商、Electron/Host 总进程、大历史、MCP、Windows/Linux 和移动远控仍需单独验收。完整测量方法、功能差异和 TODO 见 [`docs/reports/rust-node-resource-parity-2026-09-23.md`](docs/reports/rust-node-resource-parity-2026-09-23.md)。

## 环境要求

- Rust `1.95` 或更高版本
- Node.js 和 pnpm（桌面联调、集成测试及仓库脚本需要；版本以 [`mise.toml`](mise.toml) 为准）
- macOS、Linux 或 Windows

首次准备仓库依赖：

```sh
pnpm install
```

## 启动 Rust runtime

### 直接启动 App Server

先构建 Rust 二进制：

```sh
pnpm build:escode-cli-rust
```

然后通过 stdio 启动 App Server：

```sh
apps/escode-cli-rust/target/debug/escode-cli-rust app-server --stdio \
  --cwd "$PWD" \
  --data-dir "$PWD/.escode-runtime/rust"
```

App Server 的 stdout 只输出协议帧，诊断信息写入 stderr。`--cwd` 指定工作区，`--data-dir` 指定 Rust 独立数据目录；未指定数据目录时默认使用 `~/.escode/rust`。也可以直接使用 Cargo：

```sh
cargo run --locked --manifest-path apps/escode-cli-rust/Cargo.toml -- \
  app-server --stdio --cwd "$PWD" --data-dir "$PWD/.escode-runtime/rust"
```

直接运行时可以通过 `--config /absolute/path/model.json` 指定单模型配置，并在环境变量中提供配置所需的 API key。通常接入桌面 App 时不需要手写模型配置，App 会提供现有的 Provider Registry 和账号设置。

### 接入桌面 App

使用仓库脚本构建并启动 Electron，同时显式选择 Rust runtime：

```sh
pnpm dev:desktop:escode-cli-rust
```

默认构建 release 二进制。调试 Rust 代码时使用 debug 构建：

```sh
pnpm dev:desktop:escode-cli-rust --debug
```

需要隔离实验数据或使用 fixture 模型时：

```sh
pnpm dev:desktop:escode-cli-rust \
  --config /absolute/path/model.json \
  --data-dir /absolute/path/rust-experiment
```

该脚本会设置 `ESCODE_AGENT_SERVER_RUNTIME=escode-cli-rust`，由 Host 启动 Rust App Server。安装包默认已是 Rust runtime（找不到随包二进制会直接报错，不会静默回退 Node）；monorepo 开发态默认保持 Node，未执行该脚本时 `pnpm dev:desktop` 走 TypeScript runtime。

## 构建、检查与测试

```sh
pnpm build:escode-cli-rust  # 构建 debug 二进制
pnpm check:escode-cli-rust  # 边界检查、格式检查和 Clippy
pnpm test:escode-cli-rust   # Rust 单测及 App 集成测试
```

需要 release 产物时：

```sh
cargo build --locked --release --manifest-path apps/escode-cli-rust/Cargo.toml
```

## 目录说明

| 目录                                                         | 作用                         |
| ------------------------------------------------------------ | ---------------------------- |
| `apps/escode-cli-rust/src`                                    | Rust CLI 组合根和启动参数    |
| `apps/escode-cli-rust/crates/protocol`                        | App stdio/V4 协议            |
| `apps/escode-cli-rust/crates/core`、`core-api`、`domain`      | Session 核心及稳定接口       |
| `apps/escode-cli-rust/crates/model`、`tools`、`state`、`host` | 模型、工具、存储和宿主适配器 |
| `apps/escode-cli-rust/crates/app-server`、`tui`               | App Server 与终端前端        |
| `docs/specs/`                                                | 架构、迁移和验收规格         |

完整能力、配置项、数据导入和已知限制见 [`apps/escode-cli-rust/README.md`](apps/escode-cli-rust/README.md)。架构边界见 [`docs/specs/rust-cli-architecture.md`](docs/specs/rust-cli-architecture.md)。 Node.js 与 Rust 的资源对比和功能 TODO 见 [`docs/reports/rust-node-resource-parity-2026-09-23.md`](docs/reports/rust-node-resource-parity-2026-09-23.md)。
