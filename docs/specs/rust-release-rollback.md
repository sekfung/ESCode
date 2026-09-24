# Rust runtime 发布与回退（WP10 前置）

2026-09-24。本文定义 runtime 切换面、回退步骤与验收；**当前默认仍是 Node runtime**，只有下列门槛全部通过才切换。

## 切换面

App 通过 Host 启动参数选择 Agent runtime（`packages/services/src/zcode-agent/zcodeAgentProcessManager.ts`）：

| 变量                           | 作用                                                                                                                                                                                                                                   |
| ------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ZCODE_AGENT_SERVER_COMMAND`   | Agent 可执行文件；指向 Rust 二进制即启用 Rust runtime                                                                                                                                                                                  |
| `ZCODE_AGENT_SERVER_RUNTIME`   | `zcode-cli-rust`：有 `ZCODE_AGENT_SERVER_COMMAND` 时用该命令，否则用随包 `resources/glm/zcode-cli-rust[.exe]`（缺失则回退 Node 并告警）；均附加 `--cwd <workspacePath>` 与 process 存储握手。`node`：显式使用 Node（回退）。其他值报错 |
| `ZCODE_AGENT_SERVER_ARGS_JSON` | 附加参数；Rust 模式下不允许自带 `--cwd`（Host 注入）                                                                                                                                                                                   |

未设置这些变量（或设为 `node`）时，Host 启动随包分发的 Node CLI——这就是回退路径。随包 Rust 二进制由 `ZCODE_BUNDLE_RUST_AGENT=1` 的打包流程产出（见 rust-packaging.md），默认不随包。

## 回退步骤（可执行）

1. 移除/清空 `ZCODE_AGENT_SERVER_RUNTIME`（以及只想用 Node 时的 `ZCODE_AGENT_SERVER_COMMAND`）；
2. 重启 App（或重建窗口的 Local Host）；
3. 确认 Agent 进程是 Node CLI（`zcode-agent` 日志中的 runtime 事件）。

回退**不需要**数据迁移：Rust runtime 只读 TS 存储（见下），Node 继续使用原有 `ts.sqlite`。

## 数据边界与已知限制

- Rust 导入 TS 数据时只读源库并以备份方式复制（`crates/state/src/legacy_storage.rs`），源库与 TS 附件目录逐字节不变，因此回退无损。
- Rust 自己的会话写在 `<dataDir>/rust-sessions.sqlite`。**回退到 Node 后这些会话在 Node 侧不可见**（Node 不读该库）；数据仍在磁盘上，重新启用 Rust 即可再次打开。若产品要求回退后仍能看到 Rust 期会话，需要新增导出/反向迁移，当前未实现，属已知限制。
- workspace 独占锁（`workspace-<hash>.lock`）在进程退出后释放；回退或重启后另一 runtime 可立即接管同一 workspace。
- 同 data dir 的并发导入/写入已修复：导入事务改为 IMMEDIATE，避免 DEFERRED 升级失败（SQLITE_BUSY_SNAPSHOT）。

## 发布门槛（全部通过才切换默认 runtime）

| 维度     | 验收方式                                                                           | 现状                                                                                                                                                                                                                                  |
| -------- | ---------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 功能对齐 | App 集成套件 + 各能力差分语料（权限、Bash、代理、prompt 等）                       | Windows 上 196 用例 194 通过、2 跳过（理由见 rust-windows-parity.md）；**macOS/Linux 未跑**                                                                                                                                           |
| 性能     | 首段延迟、会话加载、空闲驻留、大历史查询（见 rust-runtime-performance.md）         | Linux 协议开销/内存（rust-perf-2026-09-24）；**真实模型服务（自建 DeepSeek，20 轮交替）冷首轮 3678→327ms、热轮 537→297ms（p50），p95/p99 见 rust-perf-2026-09-25-real-provider**；公网供应商、长回复、大历史、三平台 release 构建未测 |
| 数据迁移 | TS 导入用例 + 源库字节不变 + 大库实测 + 真实 runtime 产出的数据 + 真实用户数据演练 | 合成大库、真实 Node runtime 写出的会话均通过；**本机真实用户数据只读演练（3 会话）与 Node 逐会话一致**，演练发现并修复 4 处导入差异；其他机器/更大真实库需用 `zcode-cli-rust-real-data-rehearsal.test.ts` 复核                        |
| 跨平台   | Windows 已完成；macOS/Linux 需各自构建并跑同一套件                                 | **仅 Windows**（且为 GNU 目标，MSVC 未跑）                                                                                                                                                                                            |
| 发布回退 | 本文两条自动化用例 + 回退步骤演练                                                  | 用例通过；**未做真实发布演练**                                                                                                                                                                                                        |

补充硬性要求：发布验收必须用 MSVC 目标构建（本机当前缺 MSVC，只能用 GNU 目标得出源码级结论）。

## 自动化验收（已实现）

`packages/services/tests/zcode-cli-rust-rollback.test.ts`：

1. `Rust import leaves TS storage byte-identical and frees the workspace for rollback`
   - 导入后 `ts.sqlite` 的 sha256 与导入前一致；TS store 仍可读到原会话；
   - Rust 进程退出后可再次启动并读到会话（owner lock 已释放）；
   - 第二轮运行后源库仍逐字节不变。
2. `Rust storage stays readable after the process exits`
   - `rust-sessions.sqlite` 在进程退出后可直接读取，结构化事实（mode 等）确实落盘。

## 两个 runtime 的能力声明差异（2026-09-24 差分实测）

`packages/services/tests/zcode-cli-rust-differential.test.ts` 用同一场景（一次 Read 工具调用的完整 turn）
分别驱动 Node 与 Rust，比对协议投影。结论：

| 字段                             | Node   | Rust                      | 说明                                                                        |
| -------------------------------- | ------ | ------------------------- | --------------------------------------------------------------------------- |
| `independentPlanState`           | `true` | `true`（2026-09-24 起）   | 两侧一致；plan 审批三种应答与 EnterPlanMode 已差分对齐（rust-plan-mode.md） |
| `permissionModes`                | 不声明 | `["yolo","build","edit"]` | Rust 显式声明支持的模式；Node 省略，Host 对缺席字段按「都支持」处理         |
| `workspaceExecutionCapabilities` | 不声明 | `true`                    | Rust 能提供 workspace 执行能力块（`includeExecutionCapabilities`）          |
| `accountProviderConfig`          | 不声明 | `true`（有 registry 时）  | Rust 显式声明账号 provider 配置能力                                         |

行级投影（行种类、工具名、工具状态）两侧完全一致，schema 校验两侧均无错误。

### 差分用例发现并已修复的不一致（2026-09-24）

| 场景                 | Node                                                         | Rust 原行为               | 处理                                                     |
| -------------------- | ------------------------------------------------------------ | ------------------------- | -------------------------------------------------------- |
| 工具失败的错误码     | `tool_execution_failed`（contracts `ToolExecutionFailed`）   | `fault.tool.failed`       | 改用 TS 码                                               |
| 未指定 mode 的会话   | 默认 `build`，写文件先确认                                   | 默认 `yolo`，**直接写入** | 默认 `build`；移除 yolo 时代的 compact/队列/goal 门禁    |
| 权限弹窗 `summary`   | 判定原因（如 `Tool has side effects and requires approval`） | `Allow {tool}?`           | `ask_reason` 按 ruleId 取 TS 原文，矩阵生成器导出 oracle |
| 权限弹窗「完全访问」 | 主会话提供 `fullAccessOption`，选中后会话切 yolo 并放行      | 无该选项                  | 实现（排队输入一并切 yolo，与 ACK 同一次提交）           |
| 被拒工具行           | `cancelled`，无输出/错误                                     | `error` + 拒绝文案输出    | `ToolDone.denied` 收口为 `cancelled`；模型侧拒绝文案不变 |

| session/list `traceId` | 会话创建即分配 | 仅 shared context 会话有 | 新会话与子代理创建即分配 |
| 文本附件进入模型请求 | 伪 Read 调用的独立 system-reminder，位于正文之前 | 拼进用户消息、自拟文案、64 KiB 截断 | 对齐 TS 形态，五种文件形态逐字一致（rust-attachment-prompt.md） |

stop 中断流式回复后的收口相位、行与下一轮完成情况两侧一致（无需修复）。

### 仍存在、未纳入断言的差异

- 工具行字段：Node 带 `visibility`、`assistantResponseId`、结构化 `input`；Rust 只有 `inputText`。
  前端以 `inputText` 为准时无影响，但严格对齐前不能宣称行投影逐字段一致（待逐字段差分）。

该用例把上述差异写成**契约**：出现新的差异键即失败，从而在后续改动中持续守住功能对齐。
新增能力位必须同步更新本表与用例。
