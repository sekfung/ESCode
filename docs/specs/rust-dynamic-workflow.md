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
   **前置待决**：诊断与类型结论来自 TypeScript **类型检查器**（见下节），不是"Rust JS 解析器"能对齐的；
   本期的实现形态需要先在两条路线里选定（随二进制附带 Node 运行既有分析器，或自研 TS 兼容检查器）。
4. 引擎、调度与 journal：run 状态机、actor、ask、结算与持久化；以 TS 引擎为 oracle 的事件序列语料。
5. 执行器：`ScriptExecutor` 接口与 Node vm 子进程实现（子进程源由 TS 生成为资产），NDJSON 桥接与超时/取消/崩溃结算。
6. 运行工具：CreateWorkflow、EvalWorkflowSnippet、ListWorkflowRuns、GetWorkflowRun、ResumeWorkflowRun、AmendWorkflow、ListModels；
   actor 的 ask 以子会话执行；App 差分。
7. 投影与入口：V4 `workflowRuns` 状态键与增量、`workflowRunDeltas` 能力位、`/workflow` 命令、`dynamic-workflows` 技能、
   run 观测、产物发布、导入。

## 验收总则

- 每期先有 TS oracle（真实 TS 模块 + 脚本化依赖）或 App 差分，再实现；不以 Rust 自测代替对齐。
- 全部完成前 Rust 继续不宣告 `workflowRunDeltas`、不注册未实现的工具，不以空结果冒充完成。

## 第 3 期前置：诊断与类型结论来自 TS 类型检查器（2026-09-30 复核）

原始分期把第 3 期写成"JS 解析用 Rust 解析器"，复核源码后这条前提不成立：

- `dynamic-workflow/src/compiler/compile.ts`：`createWorkflowProgram` 建的是 **TS Program**
  （虚拟 host：包裹后的脚本文本 + facade `.d.ts` + 内嵌 stdlib `TS_LIBS`），
  `collectDiagnostics = getSyntacticDiagnostics() + getSemanticDiagnostics()`，即**语义诊断**（类型检查），
  且带自定义改写（如 TS1184 的 `declare`/`export` 文案）。
- 分析层直接读**类型检查器**：`checker.` 出现在 facade-misuse(15)、artifact-types(7)、schema/emit(7)、
  taint(5)、callbacks(4)、sites/state/assign/interpret 等；`getTypeAtLocation` 用于站点与产物类型判定。
- 因此"诊断与 lowered 输出逐字对齐"要求复刻 TS 的语义检查与 `.d.ts` 环境（lib.es2022 + facade），
  这是 Rust JS 解析器（oxc/swc 等）不提供的能力。

两条路线（需要用户决定，因为这会改动"Rust 原生实现工作流"的边界）：

1. **随二进制附带 Node 跑既有 TS 分析器**（推荐）：与第 5 期执行器同一条边界决定（2026-09-28 用户决定 B：
   远程/无界面环境随二进制附带 Node，只供工作流脚本使用；桌面端复用 Electron 内置 Node）。Rust 侧仍是
   runtime 进程与唯一的状态所有者，分析器/执行器是**子进程沙箱**，像 Bash 跑命令一样；NDJSON 桥接，
   诊断/降低/taint 结论天然逐字等于 TS（同一份代码）。代价：JS 脚本路径仍要求 Node 在场（执行器已如此）。
2. **自研 TS 兼容检查器**：等价于重写 TS 的类型检查器 + lib.es2022 + 诊断码/文案，工作量与风险都远超本项目
   其余各期之和，且与"以 TS 为 oracle 逐字对比"的验收方式天然冲突（两边都会漂移）。

在决定落地前，第 3 期不开工；不因此改动 App 可见行为（今日工作流工具仍按未实现处理）。

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
- 2026-09-30 GUI 中枢 `workflows/list|get|updateMeta|delete|move` 已落地（workspace 级、无会话，
  每次现扫目录，定向 scope；失败面按协议 schema 收口，不泄漏文件路径）。`runs` 是 run 历史，
  依赖第 4 期的 journal，**暂不实现**（未知方法照旧报错，不用空页冒充）。验收：App 差分
  `packages/services/tests/zcode-cli-rust-saved-workflow-hub.test.ts`（五方法逐字一致 + 落盘文件
  与目录状态一致）。
- 2026-09-30 `workflows/runs` 与 dwf journal 读面提前落地（第 4 期的存储底座）：`state` 建
  `0019_dwf_journal` 的四张表与索引（与 TS 同一 DDL），`domain::dwf_journal` 负责物理列→逻辑
  记录的解码（stopped/errored/interrupted、信封嗅探失败退化成 user）、产物归并（同 id 版本、
  失败发布、预置看板 itemCount、primary 置前）与协议行投影，core 在 `workflows/runs` 里按
  scope 决定是否下推 cwd、多取一条判 `truncated`。验收语料
  `scripts/generate-zcode-cli-rust-dwf-journal-corpus.mjs` 用**真实 TS session store 建库**（迁移即
  DDL）、插行、再跑真实 `listSavedWorkflowRunsOp`，Rust 用同一份 DDL/行重建库后逐字比对 7 组查询。
  写入方（引擎）与其余 run 内省方法（ListWorkflowRuns / GetWorkflowRun）仍属第 4 期。
- 2026-09-30 `ListModels` 已接入（第 6 期前置的只读发现面）：model crate 的注册表快照新增
  **目录面**（`providerId/modelId/providerLabel?/reasoningLevels/defaultReasoningLevel?/contextWindow?`，
  与 picker 面同源但字段集独立；`providerLabel` 保持「没取过名就缺席」），core 在轮次开始时取一次
  目录、`ListModels` 由 owner 侧直接应答（模型面 + `list_models` display + 结构化输出；没有注册表时
  按 TS 的 `model_catalog_unavailable` 报业务失败，不静默回空列表）。可见性仍由第 1 期开关裁剪。
  验收：App 差分 `packages/services/tests/zcode-cli-rust-list-models-tool.test.ts`（灰度为开、注册表
  两个模型带档位表与上下文窗时，模型面、display 与行状态逐字/逐值一致）。
- 2026-09-30 `ListWorkflowRuns` 的读面已落地（第 6 期前置）：journal 查询补 `script_text` /
  `resumed_from`，`domain::workflow_run_list` 实现标签派生（name → 脚本首个非空行截 80 UTF-16 且不留
  孤立代理项 → runId，来源恒 name/script）、归属与 `possiblyInterrupted`（journal 说没结束且非本会话）、
  停止原因与 lineage（只对 stopped 生效）、ISO 时间戳与属性式模型面 + `list_workflow_runs` 透传
  display。实现的是**注册表为空**那一支（活 run 注册表随引擎第 4 期）。
  验收：语料在既有 TS 建库基础上，新增「内省端口（`createRunIntrospectionMethods`）→ 真实工具
  handler → 模型面/display」的 `listCases`，Rust 用同一份 DDL/行重建库后逐条比对。
  工具定义与派发已接线：schema/描述取自同一套生成资产（TS 注册顺序里在保存/模型目录之前），
  `statuses` 过滤下推 SQL（`stopped`/`errored` 与 TS 同谓词：物理 `failed` 靠 failure_json 的
  `$.code` 分辨 `Interrupted`），journal 读经 `Event::WorkflowRunList` 交回会话 owner（工具在
  回合里执行，存储不在它的手里）。验收另有 App 差分 `zcode-cli-rust-list-workflow-runs-tool.test.ts`：
  两侧各自的库（Node `ts.sqlite`、Rust `data/rust-sessions.sqlite`）播同一组 run 行后，模型面与
  display 逐字一致，并覆盖 `statuses: ["stopped"]` 的下推面。

## 第 3 期路线已定（2026-10-02 用户决定）

采用路线 1：**附带 Node 跑既有 TS 分析器**。Rust 仍是 runtime 进程与唯一状态所有者；分析器与第 5 期执行器
同为 Node 子进程沙箱（桌面复用 Electron 内置 Node，`ELECTRON_RUN_AS_NODE=1`；远程/无界面随二进制附带 Node），
NDJSON 桥接。诊断、lowered 输出与 taint/causality 结论直接来自同一份 TS 代码，验收以 TS 直调结果逐字比对桥接结果。
第 3 期据此开工，SaveWorkflow 随之接入。
- 2026-10-02 第 3 期分析桥已落地：Node CLI 隐藏子命令 `__zcode-workflow-analyzer`（`cli/src/workflow-analyzer-command.ts`，
  在导入 `run` 之前分派，只加载分析器）NDJSON 一问一答，结果是 `analyzeWorkflowScript` 的 JSON 形（`core` 经
  `encodeAnalysisCore`）。Rust 客户端 `tools/src/workflow_analyzer.rs`：经 Host 的 Node 启动器
  （`ZCODE_PLUGIN_HOST_EXEC_PATH` / `_ENTRYPOINT`，`ELECTRON_RUN_AS_NODE=1`）常驻、串行，60 s 超时 / 崩溃即杀掉重拉，
  保留 TS 的单槽记忆。验收：`scripts/generate-zcode-cli-rust-workflow-analysis-corpus.mjs` 以内置技能示例 + 编译错误
  样例生成 TS 直调语料，Rust 经桥逐字比对（12 例全部一致）；`test:zcode-cli-rust` 加 `--check` 防漂移，并在
  CLI 产物就绪后带启动器单独跑桥接用例。SaveWorkflow 接入是下一步。
- 2026-10-02 内置技能包（`bundled-skills/dynamic-workflows`）接入 Rust 技能发现：`tools/src/bundled_skills.rs` 沿 TS
  `candidateBaseDirs` 同款候选（`ZCODE_OFFICIAL_PLUGINS_BASE_DIR`、Host 给的 Node 入口目录、二进制目录、cwd）×
  `packages/bundled-skills` / `../bundled-skills` / … 查找，三个必需文件缺一即拒绝整包；作为 `system` scope 根排在
  插件根之后。`SkillCatalog::response`（`skills/referenceCatalog`）排除 `system`，与 TS 按 `source: "bundled"` 排除同义；
  动态工作流关闭的会话在固化技能目录时去掉它（core `freeze_skills`，TS `collectDynamicWorkflowDisabledSkillPaths`）。
  验收：`zcode-cli-rust-bundled-skills.test.ts`（开启态模型可见、关闭态不可见、引用面板不含，两侧一致）。
  这是 SaveWorkflow 技能门的前置（门要求会话里成功加载过 `dynamic-workflows`）。
- 2026-10-02 **SaveWorkflow 已接入**（第 2/3 期合流）：`tools/src/save_workflow.rs` 按 TS 生命周期实现——validateInput
  （名字、唯一来源、内联脚本不得自带元数据块）→ resolveInput（技能门 → `script_path` 读成正文并丢掉元数据块 →
  回填 `path` / `overwrite` / `shadowing`）→ prepareApproval（经 Node 分析子进程编译，干净才问）→ handler（再编译，
  编不过回 `L{line}:C{column}` 诊断且不落盘；干净则写盘、按写的那一刻判定覆盖）。core 新增通用预处理钩子
  `ToolPort::prepare_tool`：拒绝以 `<tool_use_error>` 交回模型、不请求权限；放行时把入参换成执行事实（确认窗与
  handler 读同一份），审批门 proceed 经 `Event::Permission.approval_proceed` 让 ask 直接放行（deny 仍生效）。
  技能门按模型可见历史判定（`domain::skills::loaded_in_history`，TS `sessionHasLoadedSkill`）；确认选项对
  Create/Amend/SaveWorkflow 去掉「总是允许」（TS `askOptions.allowAlways: false`）。工具 schema 与描述由生成脚本产出。
  验收：`zcode-cli-rust-save-workflow.test.ts`（技能门、三种入参拒绝、编译诊断、新建 / 覆盖 / 草稿存全局三次确认，
  模型面结果、确认窗载荷与落盘内容两侧逐字一致）；相关对比用例 37/37（1 跳过）。已知：子代理停止用例
  （`zcode-cli-rust-subagents.test.ts`）在本机内存紧张的分组运行里偶发贴超时失败，单独与复跑均通过。
