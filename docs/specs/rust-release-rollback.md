# Rust runtime 发布与回退（WP10 前置）

2026-09-24。本文定义 runtime 切换面、回退步骤与验收。**2026-10-03 门槛全部通过、经用户决定，默认已切换为 Rust runtime**（见文末）。

## 切换面

App 通过 Host 启动参数选择 Agent runtime（`packages/services/src/escode-agent/escodeAgentProcessManager.ts`）：

| 变量                            | 作用                                                                                                                                                                                                                                    |
| ------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ESCODE_AGENT_SERVER_COMMAND`   | Agent 可执行文件；指向 Rust 二进制即启用 Rust runtime                                                                                                                                                                                   |
| `ESCODE_AGENT_SERVER_RUNTIME`   | `escode-cli-rust`：有 `ESCODE_AGENT_SERVER_COMMAND` 时用该命令，否则用随包 `resources/glm/escode-cli-rust[.exe]`（缺失直接报错，不回退）；均附加 `--cwd <workspacePath>` 与 process 存储握手。`node`：显式使用 Node（回退）。其他值报错 |
| `ESCODE_AGENT_SERVER_ARGS_JSON` | 附加参数；Rust 模式下不允许自带 `--cwd`（Host 注入）                                                                                                                                                                                    |

打包态未设置这些变量时默认启动随包 Rust 二进制；只有显式 `ESCODE_AGENT_SERVER_RUNTIME=node` 才走 Node 回退。随包 Rust 二进制由 `ESCODE_BUNDLE_RUST_AGENT` 控制（默认随包，`0` 关闭；见 rust-packaging.md）。

## 回退步骤（可执行）

1. 移除/清空 `ESCODE_AGENT_SERVER_RUNTIME`（以及只想用 Node 时的 `ESCODE_AGENT_SERVER_COMMAND`）；
2. 重启 App（或重建窗口的 Local Host）；
3. 确认 Agent 进程是 Node CLI（`escode-agent` 日志中的 runtime 事件）。

回退**不需要**数据迁移：Rust runtime 只读 TS 存储（见下），Node 继续使用原有 `ts.sqlite`。

## 数据边界与已知限制

- Rust 导入 TS 数据时只读源库并以备份方式复制（`crates/state/src/legacy_storage.rs`），源库与 TS 附件目录逐字节不变，因此回退无损。
- Rust 自己的会话写在 `<dataDir>/rust-sessions.sqlite`。**回退到 Node 后这些会话在 Node 侧不可见**（Node 不读该库）；数据仍在磁盘上，重新启用 Rust 即可再次打开。2026-09-26 用户确认：该单向模型满足发布门槛，不实现 Rust→TS 反向导出；切换前的历史回退无损。
- 2026-10-12：已删除运行期的自动回退（二进制缺失 / 就绪前失败 → Node），只保留上文的显式回退步骤；见 rust-packaging.md「不回退 Node」。
- workspace 独占锁（`workspace-<hash>.lock`）在进程退出后释放；回退或重启后另一 runtime 可立即接管同一 workspace。
- 同 data dir 的并发导入/写入已修复：导入事务改为 IMMEDIATE，避免 DEFERRED 升级失败（SQLITE_BUSY_SNAPSHOT）。

## 功能缺口范围（2026-09-25 用户决定）

| 缺口                                       | 决定                                                                                          |
| ------------------------------------------ | --------------------------------------------------------------------------------------------- |
| 动态工作流                                 | 已实现（2026-10，TS 引擎跑在工作流宿主、Rust 驱动；见 rust-dynamic-workflow.md）              |
| Cron 工具（CronCreate/List/Update/Delete） | 实现，与 Node 对齐                                                                            |
| ReadSessionContext                         | 完整对齐：跨 workspace 查 Rust 库，未导入的会话只读回落 TS 库；注入 `#sess_*` 引用提醒        |
| output style / 自定义 system               | 无需实现：App stdio 链路没有入口（插件 `outputStyles` 仅诊断，`systemPrompt` 只供工作流使用） |
| 自定义斜杠命令                             | 已实现，与 Node 差分一致（rust-custom-commands.md）                                           |
| 主代理记忆索引 / 提取                      | 已实现，与 Node 差分一致（rust-project-memory.md）                                            |
| MCP OAuth、媒体 Read（大图缩放、PDF 等）   | 实现                                                                                          |

## 发布门槛（全部通过才切换默认 runtime）

| 维度     | 验收方式                                                                           | 现状                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| -------- | ---------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 功能对齐 | App 集成套件 + 各能力差分语料（权限、Bash、代理、prompt 等）                       | 三平台 CI 243 用例通过、Edit 宽松匹配已与 TS 差分对齐（rust-edit-matching.md，2026-09-25）；模型可见工具面（名称/顺序/描述/embedded search 与 Bash prelude）已与 Node 请求逐字对齐（rust-tool-surface.md）；WebFetch 已实现并与 Node 实网差分一致（rust-webfetch.md）；ReadSessionContext（含跨 workspace、TS 库回落、`#sess_*` 提醒、`sess_` 会话 id）已实现并与 Node 差分一致（rust-read-session-context.md）；Cron 工具与自动化轮守卫已实现并与 Node 差分一致（rust-cron.md）；自定义斜杠命令（目录、参数与 shell 展开、插件命令、失败 ACK）已实现并与 Node 差分一致（rust-custom-commands.md）；项目记忆（Memory 段、MEMORY.md 索引、记忆写入放行与来源补写、轮次后自动提取）已实现并与 Node 差分一致（rust-project-memory.md）；output style / 自定义 system 在 App 链路无入口，无需实现；媒体 Read（图片预算压缩、PDF 原生与页渲染、视频）、工具结果媒体投影、附件大图缩放与来源路径已实现并与 Node 差分一致（rust-media-read.md 第 1–4 期）；MCP 工具结果格式与图片结果（含超预算图片 artifact）已对齐（rust-mcp-parity.md 第 1–2 期）；会话标题与 Goal 摘要标题（`titleFromInput` 公式、辅助模型 sidecar、`/goal` 即时生成与兜底摘要、`custom` 优先与首条输入回退校验）已实现并与 Node 差分一致（rust-session-title.md，2026-09-26）；WebSearch（provider-native 搜索，仅声明 `supportsNativeWebSearch` 的 Anthropic 模型）已实现并与 Node 差分一致（rust-websearch.md）；MCP OAuth 授权码 + PKCE 已实现并与 Node 差分一致（授权、跨 runtime 凭据复用、refresh、invalid_grant；rust-mcp-oauth.md，2026-09-26），client_credentials 已实现并与 Node 差分一致，官方 MCP（http 身份头、stdio `_meta`）已与 Node 差分一致；**仍缺**：浏览器截图压缩（rust-mcp-parity.md 第 4 期，依赖未提供的浏览器工具）；跨日 reminder 已实现（rust-date-change.md）；差分中新发现：官方插件 seed 已实现并与 Node 双向字节一致（rust-official-plugin-seed.md）；新会话 system prompt 的 Chat 合并与 Shell 名称已对齐（rust-request-context.md）；工作流只读查询已经工作流宿主应答（`crates/core/src/app/engine_requests.rs:125-133`，2026-10-12 复查修正此前「按用户决定声明 unsupported」的表述），其余缺口的范围见下节 |
| 性能     | 首段延迟、会话加载、空闲驻留、大历史查询（见 rust-runtime-performance.md）         | 真实模型服务（自建 DeepSeek 与公网供应商）、长历史重开均已测，见 rust-perf-2026-09-25-real-provider；三平台原生 release 基准已由 CI `bench` job 实测（run 36237407731）：Linux/macOS/Windows 上 Rust 启动 8/28/114ms vs Node 990/1310/1604ms，空闲内存 12.7/11.5/8.6MiB vs 362/401/264MiB（rust-runtime-performance.md）                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| 数据迁移 | TS 导入用例 + 源库字节不变 + 大库实测 + 真实 runtime 产出的数据 + 真实用户数据演练 | 合成大库、真实 Node runtime 写出的会话均通过；**本机真实用户数据只读演练（3 会话）与 Node 逐会话一致**，演练发现并修复 4 处导入差异；其他机器/更大真实库需用 `escode-cli-rust-real-data-rehearsal.test.ts` 复核                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| 跨平台   | 三平台各自构建并跑同一套件                                                         | **Windows（MSVC）/ macOS（arm64）/ Linux CI 全部通过**；6 目标 release 与 4 个安装包构建通过（rust-ci.md）；macOS/Linux 安装包未真机运行                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| 发布回退 | 本文两条自动化用例 + 回退步骤演练                                                  | 用例通过；安装包内置 Rust 与 Node 两种 runtime 已在产物中核实；回退后 Rust 期会话在 Node 侧不可见，已由用户确认可接受（不做反向导出）；未做真实发布演练                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |

### 2026-10-03 功能对齐更新

- 动态工作流：CreateWorkflow / actor / Resume / Amend / 升级问答 / `/workflow` / run 查询、`workflowRuns` 投影（键级增量与旧界裁剪）、
  模型失败分类与进程级并发治理器均已与 Node 差分一致（rust-dynamic-workflow.md）。
- Hooks：用户 / 插件 hooks 七个事件、`hookInvocation` 行、PermissionRequest 竞速、工作区 hooks 软门禁 / 审核 / 信任授权、
  mailbox 内部 hooks 均已与 Node 差分一致（rust-hooks.md）。
- Browser / Computer Use 经 `node_repl` 实现（rust-browser-use.md）；遥测事实、本地 TTFT、用量统计、`computer-use/operation-event`
  已对齐；`v4/cua/permission-observation` 在当前产品不可达（rust-parity-remaining.md）。
- 已知残余差异各自记录在对应 spec 的「已知差异」节，均不改变协议形状。

### 2026-10-03 跨平台真机与发布回退验收（GitHub CI）

- 三平台测试（rust-runtime-ci run 37097969881，提交 1c6857c）：Linux / Windows（MSVC）/ macOS（arm64）的 cargo 全工作区测试与
  App 集成差分套件全部通过。期间修复：Linux 上目录项顺序（libuv 按字节序排序）、工作流分析语料行尾、tool-parity 基准表、
  runtime_consistency 模拟工具注册表；工作流恢复用例改为不依赖核数（3 核 runner 上并发天花板为 1）。
- 安装包真机演练（release 工作流 `installed` 作业）：Windows x64、macOS arm64、macOS x64（原生 Intel runner
  `macos-15-intel`）、Linux x64 上安装真实安装包（NSIS 静默 / dmg / deb），在空临时目录下启动（不继承仓库 checkout 的 cwd），
  确认应用持续运行 20s 且该窗口内主进程日志 `[database-startup] terminal` 的终态为 `ready`（2026-10-12 起；此前只看进程
  存活，数据库启动报 `unsupported_runtime` 时作业仍通过），再用包内 Node 与 Rust
  runtime 在同一数据目录上走 Node → Rust → 回退 Node：Rust 读到 Node 写的会话并能继续对话，TS 源库逐字节不变，回退后
  Node 原会话完好且可继续（`escode-cli-rust-installed.test.ts`）。
- 真实发布（标签 `v3.14.0-rust-rc.1`，release run 37100036941）：6 个 Rust runtime 目标与 4 个平台安装包构建、四平台演练全部
  通过后创建**草稿** Release（21 个资产）；从 Release 下载 6 个 Rust runtime 压缩包，sha256 全部一致。资产名比对因 GitHub
  把空格换成 "." 误报，已修正比对规则。

至此功能对齐、性能、数据迁移、跨平台与发布回退五项门槛均有自动化验收记录。其他机器上更大真实库的迁移复核可用
`escode-cli-rust-real-data-rehearsal.test.ts` 继续补充。

### 2026-10-03 切换默认 runtime（用户决定）

- 桌面端 `resolveDefaultESCodeAgentCommand`：未设置 `ESCODE_AGENT_SERVER_RUNTIME`、没有自定义命令且不在 monorepo 开发态时，
  默认用随包 `resources/glm/escode-cli-rust`；找不到二进制或 Rust 就绪前失败即回退包内 Node runtime（同一 manager 内
  记住失败）。开发态与自定义命令保持原 Node 语义。
- 打包默认随包 Rust runtime（`ESCODE_BUNDLE_RUST_AGENT=0` 可关闭，此时安装包只有 Node，桌面端自动用 Node）。
- 回退：设置 `ESCODE_AGENT_SERVER_RUNTIME=node` 后重启 App；数据边界不变（Rust 只读 TS 库，回退后 Rust 期会话在
  Node 侧不可见，已由用户确认可接受）。
- 草稿 Release `v3.14.0-rust-rc.1` 保留为草稿（用户决定），未公开。

### 2026-10-10 发布回退复验（对照 ESCode-rs 补齐六项之后）

`-p` 无头模式、断流恢复、重试原因码、日志保留、Bash 解析对齐、客户端 / 归因头合入后（提交 5b78d41），重新复验：

- rust-runtime-ci run 37789222331：checks 与三平台 test / bench 全部通过。
- release 工作流 run 38018557841（workflow_dispatch，不发布）：6 个 Rust runtime 目标（含 x86_64 / aarch64 MSVC）、
  4 个平台安装包、四平台安装包 Node → Rust → Node 演练全部通过；`publish GitHub Release` 按设计跳过。

默认 runtime 维持 Rust。

补充硬性要求：发布验收必须用 MSVC 目标构建——已由 GitHub CI 与 release 工作流满足（2026-09-25）。

## 自动化验收（已实现）

`packages/services/tests/escode-cli-rust-rollback.test.ts`：

1. `Rust import leaves TS storage byte-identical and frees the workspace for rollback`
   - 导入后 `ts.sqlite` 的 sha256 与导入前一致；TS store 仍可读到原会话；
   - Rust 进程退出后可再次启动并读到会话（owner lock 已释放）；
   - 第二轮运行后源库仍逐字节不变。
2. `Rust storage stays readable after the process exits`
   - `rust-sessions.sqlite` 在进程退出后可直接读取，结构化事实（mode 等）确实落盘。

## 安装包版本升级 / 降级演练（2026-11-02）

发布门槛里的「三平台安装、升级、回滚」不能只靠 runtime 切换演练：安装器覆盖旧版本、复用同一数据目录、
再降级回旧版本，是发布链路独立的失败面。本节把它固定为 `workflow_dispatch` 演练。

**输入与来源**

- 两次 `release` 工作流实跑：一个 `base`（上一发布 tag，当前为 `v3.15.2`）与一个 `head`（当前 `dev`），
  各自产出 `desktop-<os>-<arch>` 安装包 artifact；演练不重新构建，避免与发布产物分叉。两次构建必须
  使用相同的 `ESCODE_ENV=production` 身份（tag `v3.15.1` 的 release 工作流尚未固定该变量，会产出
  Preview/TEST 安装包，不能作为同应用升级基线）。base 与 head 版本号相同（例如都是 `3.15.2`）时，
  演练验证的是「上一发布 → 当前构建」的覆盖升级；版本号递增后同一流程直接适用。
- base 若早于 Rust Host 默认翻转，不强制 Host 断言；head 必须启动包内 Rust Host 并 attach renderer。
- 演练工作流 `rust-version-drill.yml` 通过 `base_run_id` / `head_run_id` 下载两套 artifact，矩阵与
  release 的 desktop 一致：`win-x64` / `mac-arm64` / `mac-x64` / `linux-x64`。
- 工作流文件必须存在于默认分支（`main`）才能 `workflow_dispatch`；`--ref dev` 只决定运行哪份提交，
  `actions/checkout` 取的是该 ref 的 `scripts/ci-install-desktop.mjs`，所以脚本修复推 `dev` 即可生效。
- 安装包路径必须绝对：`apt-get install` 会把相对路径当成包名（`E: Unable to locate package`）。

**步骤（每平台复用同一 HOME、userData 与数据目录）**

1. 安装 base 安装包并启动：数据库启动终态必须 `ready`；冻结设置夹具的已知值在启动后保持不变。
2. 用 head 安装包覆盖安装到同一路径：安装器退出码为 0；应用载荷 `resources/app.asar` 的 sha256 必须
   变化（Electron 启动器在 base/head 使用同一 Electron 版本时逐字节相同，不能作为替换证据）；再次启动需
   `ready`、包内 Rust Host 启动并 attach renderer、不得出现 Node utility 窗口 Host，设置值保持不变。
3. 再用 base 安装包覆盖安装（降级）：安装器退出码为 0；`app.asar` 的 sha256 必须回到 base 的值；
   启动仍要求 `ready` 且设置值不变。

**失败语义**

- 任一步安装失败、启动未 `ready`、Host 断言不符、`app.asar` hash 未按预期变化或设置值被改写，演练直接
  失败；不允许用重试掩盖安装器或迁移问题。
- 若未来版本引入不可降级的存储迁移，降级步骤必须按产品契约显式失败，并记录为「降级不支持」；不能把
  失败改成跳过。

**边界**

- 不验证 macOS 公证 / Windows 签名（当前发布未签名）；不替代上文「Host 实现切换矩阵」的
  Node → Rust → Node 演练，也不替代 S4-i 的远程 Source Server 主链。
- 演练用的 base/head 安装包仍带 Node runtime 与 `ESCODE_AGENT_SERVER_RUNTIME=node` 回退，因此只证明
  「安装/升级/降级」这条安装器链路，不等于删除 Node runtime 所需的「Rust-only 三平台发行门禁」。
- 实现入口：`scripts/ci-install-desktop.mjs --version-drill <base-artifacts> <head-artifacts>`。

### Rust-only 安装门禁（2026-11-02 设计）

`--rust-only` 把安装包演练从「Rust Host 可选」推进到「不依赖 Node agent bundle」：

1. 安装（或覆盖安装）后断言 `resources/glm/escode-cli-rust[.exe]` 存在，删除
   `resources/glm/zcode.cjs`；若未来安装包已不含 Node bundle，则把「不存在」视为通过，不伪造删除成功。
2. 启动真实安装包：数据库启动终态必须 `ready`，日志必须出现 Rust Host
   `started` 与 renderer `attached`，且不得出现 per-window Node utility Host。
3. `escode-cli-rust-installed-rust-only.test.ts` 只通过安装包内的 Rust 二进制创建会话、发送一轮输入并等待
   完成；不加载 `zcode.cjs`，schema 错误必须为 0。
4. `rust-version-drill.yml` 的 `rust_only=true` 把同一流程套到 base → head → 降级三个阶段；任一步
   需要 Node agent bundle、启动失败或 Host 断言不符都直接失败，不跳过、不回退。

边界：当前正式安装包仍带 Node bundle，因此本门禁先证明「移除 Node agent bundle 后应用仍可运行」；
真正把 `zcode.cjs` 从打包产物删除仍需单独的打包开关与产物检查，不能把本门禁当成产物已精简的证据。

**实跑证据（2026-11-02，四平台全绿）**

- base：`release` run `41365221620`（tag `v3.15.2`，`workflow_dispatch`），四平台 `desktop-<os>-<arch>` artifact。
- head：`release` run `41365142237`（`dev` @ `eaa3e949`），四平台同名 artifact。
- 演练：run `41387688296`（`dev` @ `c4ed38af`），`drill (mac-arm64)` `125092065485`、`drill (mac-x64)`
  `125092065666`、`drill (win-x64)` `125092065784`、`drill (linux-x64)` `125092065840` 全部 success。
  每步 `app.asar` sha256（base → head → 降级）：

  | 平台      | base            | head            | 降级            |
  | --------- | --------------- | --------------- | --------------- |
  | mac-arm64 | `5ff1f955…e757` | `433b7ad3…a5f6` | `5ff1f955…e757` |
  | mac-x64   | `63b9549a…40e8` | `31d298ba…d891` | `63b9549a…40e8` |
  | win-x64   | `459ac7a9…b4db` | `3c87c7b8…1840` | `459ac7a9…b4db` |
  | linux-x64 | `64ca5d5e…cd40` | `3d6d88ea…81d6` | `64ca5d5e…cd40` |

  三次启动均到 `database-startup terminal { status: 'ready' }`；head 阶段断言包内 Rust Host 启动并 attach
  renderer；每步后冻结设置（`locale`、`unknownFromNode.kept`）保持不变，四平台日志都以
  `version upgrade/downgrade drill passed: base → head → base, settings preserved` 结束。

- 前两次 dispatch 失败保留为根因证据：`41385073682`（apt 把相对路径当包名；演练模式 `app` 未赋值导致
  `spawn(undefined)`）、`41385559746`（`sha256(installed.app)` 残留引用）。第三次 `41386625407`
  只在 Windows 通过，暴露「启动器 exe 在 base/head 间相同」的第三处断言缺陷。
- 操作约束：工作流文件已在默认分支 `main`（`d25e3f38`）；`workflow_dispatch` 的 `--ref dev` 决定
  checkout 的脚本版本，因此持续修 `scripts/ci-install-desktop.mjs` 后重新 dispatch 即可，不必改 `main`。

### Rust-only 打包产物开关（2026-11-02 设计）

安装门禁只能证明「运行时删掉 `zcode.cjs` 仍可用」，不能证明产物本身已精简。产物级开关与门禁如下：

1. `ESCODE_BUNDLE_NODE_AGENT=0` 让打包链路不再把 `zcode.cjs` / `.node-bundle-meta.json` 放进
   `resources/glm`，改为保留独立的 `plugin-host.cjs`。官方插件仍由 Electron-as-Node 执行
   `__escode-plugin-host`，因此不能把插件宿主一起删掉。`prepare:agent-bundle` 仍构建 CLI 与官方插件
   runtime，并继续 stage 官方插件 seed、`packages/bundled-skills`、`rea-runtime` 与 sharp/koffi 原生依赖。
2. 该开关必须与 `ESCODE_BUNDLE_RUST_AGENT` 同时开启；两者同时关闭会让安装包没有任何 agent runtime，
   构建阶段直接失败，不产出无法启动的包。
3. 产物断言在两个位置执行：`electron-builder.config.js` 的 `afterPack`（本地打包即失败）与
   `assert-release-artifacts.mjs`（release 产物门禁）。Rust-only 构建要求
   `resources/glm/escode-cli-rust[.exe]` 与 `resources/glm/plugin-host.cjs` 存在，且
   `resources/glm/zcode.cjs` 与 `.node-bundle-meta.json` 不存在；所有构建都要求至少存在一种 agent runtime。
4. `release` 工作流的 `workflow_dispatch` 输入 `rust_only=true` 使用该开关构建四平台产物，并跳过依赖 Node
   agent bundle 的 Node → Rust → Node 回退演练，改为执行 Rust-only 安装、Host 启动与安装包会话门禁。
5. `ci-install-desktop.mjs` 在 Linux 上按 `resources/glm/escode-cli-rust` 反查 `glm` 目录，兼容已移除
   `zcode.cjs` 的产物；Rust-only 安装阶段只要求 Rust 二进制。
6. Windows 打包态启动诊断以「至少存在一种随包 agent runtime」为完整：Rust-only 有 Rust 二进制、
   无 Node bundle 时不报警；仅 Rust 与 Node 两者都缺失时才记录资源损坏警告。
7. 版本演练要移除 base/降级安装包里的 Node bundle。Linux apt 安装目录 `/opt/ESCode` 归 root，
   `zcode.cjs` / `.node-bundle-meta.json` 的删除在普通用户 EACCES 时走 `sudo`，避免失败或半剥离产物。

边界：该开关只负责「产物不含 Node agent bundle」。把 `ESCODE_BUNDLE_NODE_AGENT=0` 设为发布默认、删除 TS CLI、
本地 Node Host 路径与 `ESCODE_AGENT_SERVER_RUNTIME=node` 回退，仍需三平台 Rust-only 安装/升级/回滚演练与
P1 残余项收敛后再单独确认。

**实跑证据（2026-11-03，四平台全绿）**

- Rust-only 产物门禁：`release` run `41491160036`（`dev` @ `053f086a`，`workflow_dispatch`，
  `rust_only=true`）。8 个 `rust runtime`、4 个 `desktop`、4 个 `installed` 作业全部 success；四平台
  `desktop` 的 `assert release artifacts` 均通过，`installed` 均执行并通过
  `launch installed package with Rust Host`、`Host switch and rollback drill`、
  `launch installed package with Rust Host only`、`Rust-only installed runtime session`；Node → Rust → Node
  **runtime** 演练按 `rust_only` 跳过。`installed` 作业：linux `125464648850`、win `125464646488`、
  mac-x64 `125464649317`、mac-arm64 `125464649482`。
- Windows `installed (win-x64)` 首次在 NSIS 启动阶段以 `0xC0000005` 崩溃，原作业重跑后通过；失败发生在
  安装器进程启动阶段，不是产物断言失败。macOS 的 `macos-latest` 池连续排队超时后，发布与演练的 arm64
  作业固定到 `macos-14`（commit `053f086a`），随后完成。
- 版本升级/降级：`rust-version-drill` run `41503492904`（`dev` @ `83ecf28c`，`rust_only=true`），base
  `release` run `41365221620`（tag `v3.15.2`）→ head `41491160036` → 降级。四平台 job：mac-arm64
  `125480499642`、mac-x64 `125480499274`、win-x64 `125480497508`、linux-x64 `125480500515`，全部 success。
  每步 `app.asar` sha256（base → head → 降级）：

  | 平台      | base            | head            | 降级            |
  | --------- | --------------- | --------------- | --------------- |
  | mac-arm64 | `5ff1f955…e757` | `3e50dab6…b1ab` | `5ff1f955…e757` |
  | mac-x64   | `63b9549a…40e8` | `4d2a99a4…9391` | `63b9549a…40e8` |
  | win-x64   | `459ac7a9…b4db` | `ae52d073…fa12` | `459ac7a9…b4db` |
  | linux-x64 | `64ca5d5e…cd40` | `bdabee8a…fe06` | `64ca5d5e…cd40` |

  三次启动均到 `database-startup terminal { status: 'ready' }`，冻结设置保持不变，四平台日志均以
  `version upgrade/downgrade drill passed: base → head → base, settings preserved (rust-only)` 结束。
  Linux base/降级阶段对 root-owned `/opt/ESCode` 的 `zcode.cjs` 与 `.node-bundle-meta.json` 经 `sudo`
  删除成功（commit `83ecf28c`）。

- Windows 首次 run 的 NSIS 崩溃和 macOS runner 排队取消均保留为环境证据；失败原因与产物断言、Rust-only
  启动语义无关，不能据此判定产物不可发布。

## 两个 runtime 的能力声明差异（2026-09-24 差分实测）

`packages/services/tests/escode-cli-rust-differential.test.ts` 用同一场景（一次 Read 工具调用的完整 turn）
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

- 行投影字段已逐字段对齐（rust-row-projection.md，2026-09-26）。包括：
  - `visibility`、turnHeader 的 `executionKind`/`activeMs`/`historyRoundCount`；
  - userInput 的 `rootSourceCommandId`；
  - toolCall 的 `assistantResponseId`/`input`；
  - `/goal` 的 controlOnly query 轮与 goalContinuation 执行轮。
    仍存在的差异只剩 id 的取值方式：Node 的 userInput 与 assistantText 的 `entityId` 分别取 turn id 与响应 id。

该用例把上述差异写成**契约**：出现新的差异键即失败，从而在后续改动中持续守住功能对齐。
新增能力位必须同步更新本表与用例。

## Host 实现切换矩阵（阶段 3；S6 起固定 Rust Host）

本文件上文描述的是 **Agent runtime**（Rust CLI / Node CLI）的切换面。Host 进程自 S6（2026-11-04）起
不再有实现切换开关，本地与远程统一由 App 级 Rust Host 服务：

| 范围                           | 实现                                           | 回退                               | 说明                                                                            |
| ------------------------------ | ---------------------------------------------- | ---------------------------------- | ------------------------------------------------------------------------------- |
| 本地 workspace 窗口            | App 级 Rust Host（`apps/escode-host`）         | 无                                 | `ESCODE_RUST_HOST` 已删除；`ESCODE_RUST_HOST_BINARY` 只覆盖二进制路径           |
| 远程 SSH/WSL/Docker 与 Profile | App 级 Rust Host 的 Source Server scope        | 无                                 | 失败显式报错                                                                    |
| Web 服务端（`kind: "server"`） | App 级 Rust Host（`kind: "server"` transport） | 无                                 | 与 ssh/wsl/docker 共用 Source Server 注册表；连接注册表与本地窗口 Host map 隔离 |
| Agent runtime（本地/远程）     | Rust CLI                                       | `ESCODE_AGENT_SERVER_RUNTIME=node` | 见上文；与 Host 生命周期正交                                                    |

Host 回退步骤（S6 起）：

1. Host 实现没有运行时回退；需要回退时只能回滚到仍含 Node Host 的旧安装包（安装包降级）；
2. 回滚后按旧版本文档设置 `ESCODE_RUST_HOST=0` 才走 Node Host，新版会忽略该变量；
3. 新版确认主进程日志出现 `[rust-host] started`，且没有 `forked host process for (local-`。

数据边界：Host 只读写既有设置文件与 tasks-index；切换 Host 实现不迁移、不改写这两类文件。安装包演练
`node scripts/ci-install-desktop.mjs --launch-only --host-switch-drill` 复用同一 home 连续三次以
Rust Host 启动（S6 起不再演练 Node Host），断言每次 Host 类型正确，并校验冻结设置夹具（含未知键）在重启后
未被破坏。release 工作流的 `installed` 四平台作业执行该演练（`.github/workflows/release.yml`）。
2026-10-24 release CI run `40022683157` 的 `installed` 四平台作业全部 success，四个日志均出现
`host switch and rollback drill passed: node → rust → node, settings preserved`；
本地 linux-x64 当前源码包也已跑通。

golden freeze（阶段 3）：Host parity 切片已支持冻结 Node 侧观察（`scripts/diff-escode-cli-rust-parity.mjs --record`）
与只跑 Rust 的判定（`--golden`），冻结资产在 `scripts/fixtures/parity/`；settings interop 增加
“TS 写出的冻结 `setting.json` → Rust 单向读入”用例。Desktop Host oracle 已清零：全部 `packages/desktop/test/rustHost*.test.ts` 不再
`import @escode/services` / `services/src/*`，原 TS 观察冻结在
`packages/desktop/test/fixtures/golden/`（大 JSON 用 gzip，seed 库在录制前快照）；
生成器 `scripts/generate-rusthost-*-golden.ts --check`（汇总入口
`pnpm check:rust-host-golden`）检测漂移。真实 `@escode/client` 保留（客户端契约，不是 oracle）。
CLI 确定性 oracle 已清零：`packages/services/tests/escode-cli-rust-*.test.ts` 共 156 个，
正常跑只启动 Rust；观察冻结在 `packages/services/tests/fixtures/cli-golden/`
（`nodeOracle` record/load，`ESCODE_RECORD_CLI_GOLDEN=1` 重录；legacy DB 用 gzip seed）。
例外为非确定性 benchmark/rehearsal（`long-history-bench`、`real-data-rehearsal`、
`real-provider-bench`，默认跳过）与仅 Windows 的 `shell-resume-notice`（golden 待 Windows 重录）。
在清零前不删除 Node Host / TS CLI；Web 服务端目标仍保留窗口内 Node Host。
