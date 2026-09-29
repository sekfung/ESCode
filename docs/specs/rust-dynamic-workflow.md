# Rust 动态工作流

2026-09-28。用户决定在 Rust 原生实现工作流，目标是之后以 Rust 作为默认 runtime（此前 Rust 声明不支持：
不宣告 `workflowRunDeltas`、不暴露 `/workflow` 与工作流工具，见 rust-v3.14.3-drift.md）。

## 边界决定

工作流脚本是模型写的 JavaScript。TS 的做法是：对脚本的 JS AST 做静态分析与 lowering，再在 Node 子进程的
`vm` 独立 realm 里执行，脚本经 NDJSON 调用宿主 `__host.*`，由引擎（WorkflowEngine）裁决。

- Rust 负责：引擎、调度、持久化（journal）、工具、静态分析（JS 解析用 Rust 解析器）、协议与 V4 投影、Host 方法。
- 脚本执行器：放在 `ScriptExecutor` 接口之后。第一个实现是 Node `vm` 沙箱子进程（与 TS 同一隔离模型；
  桌面端复用 Electron 内置 Node，`ELECTRON_RUN_AS_NODE=1`，与现有 TS agent 相同）。
  远程/无界面环境需要可用的 node；之后可以换成内嵌 JS 引擎实现同一接口，彻底去掉 Node，其余部分不变。
- 远程/无界面环境的 Node（2026-09-28 用户决定 B）：随 Rust 二进制附带 Node，只供工作流脚本执行器使用；
  桌面端仍复用 Electron 内置 Node。发布与打包在第 5 期（执行器）一并落地。
- 选择依据：用户目标是 Rust 作为 runtime 进程；执行器只是运行脚本的沙箱，同 Bash 运行命令。

## 现状清单（TS，约 5.7 万行，不含测试）

| 区域                                                         | 位置                                                                                 | 规模      |
| ------------------------------------------------------------ | ------------------------------------------------------------------------------------ | --------- |
| 分析、lowering、引擎、调度、facade                           | `dynamic-workflow/src`（analysis/compiler/engine/facade/lowering/schema）            | ~2.1 万行 |
| 沙箱执行 harness 与子进程源                                  | `dynamic-workflow-runtime/src`                                                       | ~1.3 千行 |
| 定义、expert、生命周期、调度                                 | `core/src/workflow`                                                                  | ~5.3 千行 |
| 工作流工具与已保存工作流                                     | `core/src/tool/handlers`（create/save/amend/list/eval/runs/resume、saved-workflows） | ~6.3 千行 |
| 运行服务、提交、观测、导入、产物、git world read、驱动与并发 | `bootstrap/src/app`                                                                  | ~1.4 万行 |
| 契约与端口                                                   | `contracts/src`（workflow、dynamic-workflow-run.port、tools）                        | ~4.7 千行 |

协议面：`workspace/updateDynamicWorkflowPolicy`、`createSession/resume.dynamicWorkflowEnabled`、
`workflows/list|get|updateMeta|delete|runs|move`、V4 `workflowRun.updated|removed` 增量与 `workflowRunDeltas` 能力位、
`/workflow` 内置斜杠命令、内置技能包 `dynamic-workflows`。

## 分期（每期独立验收，按顺序推进）

1. 开关与工具面：`workspace/updateDynamicWorkflowPolicy` 与 `dynamicWorkflowEnabled`（同 OffPeak 的读法，fail-closed）；
   未实现的工作流工具在实现前不注册（保持现状），开关只决定之后各期工具的可见性。
2. 已保存工作流：存储格式与解析/序列化（TS 只有一份，Rust 逐字对齐），`workflows/*` 管理方法，
   ListSavedWorkflows / SaveWorkflow。验收：TS 存储 oracle 语料 + App 差分。
3. 静态分析与 lowering：以 TS 分析器为 oracle，逐阶段语料（诊断、lowered 输出、taint/causality 结论）逐字比对。
4. 引擎、调度与 journal：run 状态机、actor、ask、结算与持久化；以 TS 引擎为 oracle 的事件序列语料。
5. 执行器：`ScriptExecutor` 接口与 Node vm 子进程实现（子进程源由 TS 生成为资产），NDJSON 桥接与超时/取消/崩溃结算。
6. 运行工具：CreateWorkflow、EvalWorkflowSnippet、ListWorkflowRuns、GetWorkflowRun、ResumeWorkflowRun、AmendWorkflow、ListModels；
   actor 的 ask 以子会话执行；App 差分。
7. 投影与入口：V4 `workflowRuns` 状态键与增量、`workflowRunDeltas` 能力位、`/workflow` 命令、`dynamic-workflows` 技能、
   run 观测、产物发布、导入。

## 验收总则

- 每期先有 TS oracle（真实 TS 模块 + 脚本化依赖）或 App 差分，再实现；不以 Rust 自测代替对齐。
- 全部完成前 Rust 继续不宣告 `workflowRunDeltas`、不注册未实现的工具，不以空结果冒充完成。

## 进度

- 2026-09-30 第 1 期灰度开关已落地：`domain/src/dynamic_workflow.rs` 持有进程级结论与各会话固化值
  （fail-closed、创建参数优先、翻转不回收已固化会话），`core/src/app/dynamic_workflow.rs` 提供
  `workspace/updateDynamicWorkflowPolicy`、会话创建固化与工具面过滤（`DYNAMIC_WORKFLOW_TOOL_NAMES`）。
  验收：domain 单测（读法）、`tests/dynamic_workflow_policy.rs`（strict 参数与回显）、App 差分
  `packages/services/tests/zcode-cli-rust-dynamic-workflow-policy.test.ts`（Node/Rust 回显同形，关闭态
  工具名逐字一致）。工具在实现前不注册，所以**开启态**的工具面差分要等第 2/6 期工具落地。
- 2026-09-30 第 2 期存储与编解码已落地：`domain/src/saved_workflow.rs`（frontmatter 编解码、
  元数据校验、参数校验）、`domain/src/yaml_emit.rs`（对齐 TS `yaml` 缺省 `stringify`）与
  `tools/src/saved_workflows.rs`（作用域根、解析、枚举、写入、遮蔽、全局→项目搬运）。
  验收语料：`scripts/generate-zcode-cli-rust-saved-workflow-corpus.mjs`（编解码，逐字节）与
  `scripts/generate-zcode-cli-rust-saved-workflow-store-corpus.mjs`（存储，读用例全文 + 写用例
  的文件快照）；两者都纳入 `pnpm test:zcode-cli-rust` 的 `--check`。
  `invalid_yaml` 的解析器措辞与 `read_error` 的 OS 文案跨平台不同，语料只比 kind（规格已记）。
- 工具（ListSavedWorkflows / SaveWorkflow）与 `workflows/*` 管理方法需要第 1 期开关的可见性裁剪，
  且 SaveWorkflow 的诊断来自第 3 期的静态分析；为此存储层先以 `pub mod` 落地，接入时改回私有。
- 2026-09-30 ListSavedWorkflows 已按第 1 期开关接入：模型面（XML 容器、24 KiB 预算）、行级
  display（`saved_workflow_list`、元文本 2 KiB 上限）与结构化输出对齐 TS；校验为
  `crates/tools/tests/fixtures/saved_workflow_tool_corpus.json`（TS 工具条目 + display 构造）+ App 差分
  `packages/services/tests/zcode-cli-rust-saved-workflow-tool.test.ts`（灰度为开时两侧模型面与 display
  逐字一致）。其余九个工作流工具未实现，所以**开启态**的工具名清单仍与 Node 不同（Node 十个、Rust 一个），
  这是刻意的分期状态；SaveWorkflow 随第 3 期静态分析落地。
