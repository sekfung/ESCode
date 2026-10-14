# 我方差异账本（第一轮，2026-10-14）

阶段 0 交付物（`docs/specs/rust-unified-host.md` §4.7）：用差分垂直切片把"Node 现行实现 vs Rust 实现"
放在同一确定性场景下逐面对拍，产出带完整定位字段的差异清单，逐条分类"真偏移 / 已批准差异 / 已知欠账"。

## 方法

- 工具：`scripts/diff-escode-cli-rust-parity.mjs`（差分垂直切片；捕获层与差分层见同名 `-capture` / `-diff` / `-report` 模块）。
- 场景 `session-baseline`：`runtime/capabilities` → `createSession` → `v4/conversation/subscribe` →
  `sendText`（mock SSE 4 chunk，标题请求按 stream 形态分别应答）→ 等 `completedSuccess` → 400ms 静默期 → 关 stdin。
- 双侧：Node bundle `apps/escode-cli/packages/cli/dist/escode.cjs`
  （sha256 `230466f56aba009d…`）与 Rust release 二进制（sha256 `94bbc0a7547dcbcc…`）；
  同一临时 HOME/workspace/storage、同一 mock provider、固定输入（clientId/commandId/文本）。
- 产物：`.escode-runtime/parity/session-baseline.{evidence.json,report.md,node|rust.frames[.raw].ndjson,node|rust.modelRequests.ndjson}`。
- 判定：**已批准**（登记在 `scripts/diff-escode-cli-rust-parity.approved.json`，含 reason 与 removal）→
  **open**（已登记未修复，`--strict` 判失败）→ **已知欠账**（`scripts/diff-escode-cli-rust-parity.baseline.json`
  指纹棘轮，只拦新差异）→ **新差异**（失败）。
- 归一化（全部登记在 evidence 的 `normalization`）：temp-root / uuid / iso-timestamp / epoch-ms /
  storage-id / runtime-token-ordinal / runtime-id-token / timing-fields / json-key-order，
  键序归一前的字节级差异行数同时记录（本次 frames 55、modelRequests 2），不静默丢弃。

分类字母沿用对标仓库 audit 的 M/T/C/S/A/F 序列，此处自含定义：
**M** 模型可见面（请求体/提示词/schema/工具结果）｜**T** 传输与帧（V4 wire、通知流、序号、投递形态）｜
**C** 宿主-RPC 契约（方法/字段集/错误码）｜**S** 持久化与存储｜**A** 应用行为与时序｜**F** 文件与副作用。

## 对拍面与级别声明（§4.6）

| 面                         | 级别          | 说明与降级理由                                                                                                                                               |
| -------------------------- | ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| frames（消息帧，位置对拍） | L4 有序全字段 | 键序归一已登记（Rust serde_json 的 Map 按 BTreeMap 排序、TS `JSON.stringify` 按插入序；两侧消费者都做 JSON 解析，键序不是契约）。位置顺序严格，禁止重排/合并 |
| storageProgress            | L2 投影级     | 只比阶段集合与每阶段键集合；条数/步骤明细是进度事实（见 T-2），时序另记                                                                                      |
| streams（遥测/进程通知）   | L3 集合级     | 比「方法×kind」集合；条数随冲刷时机波动，不参与判定（§4.6 要求的因果序断言以 evidence.timing 记录）                                                          |
| modelRequests              | L4 有序全字段 | JSON 键序归一后逐字段；提示词/schema 等字符串值本身逐字节（与现有 176 个字节级用例同源口径）                                                                 |
| workspaceFiles             | L5 字节级     | 场景后 workspace 文件清单 + 内容 sha256                                                                                                                      |

## 账本

| #   | 系列 | 面/定位                                            | 判定                            | 说明                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              | 移除/收紧条件                       |
| --- | ---- | -------------------------------------------------- | ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------- |
| T-1 | T/C  | frames #0 `$.result`                               | 已批准                          | Rust 在 `runtime/capabilities` 声明 `workspaceExecutionCapabilities`/`accountProviderConfig`（spec：`rust-app-execution-modes.md`），Host 已消费（`escodeAgentService.ts:1545`、`:4224`）；TS CLI 未声明，宿主按 `undefined` 走旧形状。方向是 **Rust 超前**                                                                                                                                                                                                                                       | TS 补齐声明或 Host 不再消费时复核   |
| T-2 | A/T  | storageProgress                                    | 已批准                          | 存储准备进度粒度：Node 逐步（checking×2+migrating×22+committing+ready，带 migration 明细），Rust 粗粒度（checking/ready，其路径无迁移步骤）。消费者只按 `phase` 计时/失败判定（`escodeProtocolClient.ts:334`）                                                                                                                                                                                                                                                                                    | Host 依赖具体步骤/数量时对齐        |
| T-3 | T    | streams                                            | open（真偏移；**归阶段 2 P2**） | Node 发 `process/mcpTelemetry`（`process_start`、`session_startup`），Rust 未发。消费方=桌面 ARMS 埋点（`packages/desktop/src/main/desktopMcpTelemetry.ts` → `perf_mcp_process_start` 等自定义事件），**非功能契约**；实现需 Rust 侧 MCP 进程实例账本（mcpId/mcpInstanceId/isolation/source + crash 上报），与计划 P2「MCP 进程账本/遥测」同包，随阶段 2 落地。                                                                                                                                   | 阶段 2 随 MCP 进程账本实现后移除    |
| C-1 | C/A  | frames（偏好请求位置与发起时机）                   | ✅ 部分收敛（2026-10-14）       | Rust 改为**创建期发起** `runtime-materialization` 偏好请求（不等待回包，避免把往返放进创建响应路径）：单元级对齐达成——回合开始前回包入缓存。请求帧位置仍在响应帧之后出站（引擎 outbox 在响应帧之后冲刷），已登记为已批准差异 `runtime-prefs-request-position`（含移除条件）。                                                                                                                                                                                                                     | 位置差异见登记表                    |
| C-2 | C    | frames（命令响应）与遥测事实                       | ✅ 部分收敛（2026-10-14）       | 遥测事实：Rust 全部 `v4/telemetry/event` 事实（首条 `turn.started` 起）带 `memoryEnabled`（取自物化偏好回包；shared schema 默认 false）——UI 埋点消费 `fact.memoryEnabled`，差分用例 `escode-cli-rust-telemetry-facts.test.ts` 增加该字段投影后两侧一致通过。命令响应：inputAccepted 等后续 ack 已带；createSession ack 缺（拿不到值不能写字段），登记为已批准 `create-session-ack-memory-enabled-absent`（无 Host 消费方）。                                                                      | 创建 ack 差异见登记表               |
| C-3 | C    | frames（subscribe ack）                            | 已批准                          | Node ack 多出可选诊断块 `openTiming{initialFrameEncodeMs,sessionRuntimeState,snapshotRowCount,…}`；shared schema 为 optional、宿主条件合并（`escodeAgentService.ts:5967-5968`）                                                                                                                                                                                                                                                                                                                   | Host 要求打开耗时分解时 Rust 补发   |
| T-4 | T    | frames #8–#13 `$.params.frame.fromSeq` 等          | 真偏移（欠账）                  | 会话日志 seq 编号不同（Node 0/1/3/5/18… vs Rust 5/6/9/11/15/19）；Node 另发独立 `state.updated` 通知，Rust 把状态并入帧 deltas；末尾帧数差 1                                                                                                                                                                                                                                                                                                                                                      | 与阶段 2 P2「投递层状态机对齐」合流 |
| M-1 | M    | modelRequests #0/#1 `$.body.max_completion_tokens` | ✅ **已修复（2026-10-14）**     | 根因：TS 侧请求体由 AI SDK 组装，maxOutputTokens 只经模型 option map 落字段（默认映射 `{'max_completion_tokens': maxOutputTokens}`）；Rust 手写正文硬编码 `max_tokens`。修复：`crates/model/src/model_protocol.rs` Chat 路径去掉硬编码字段，映射命中优先、未命中回退 TS 默认字段名。验证：parity 复跑两侧请求体键集合与值一致（`max_completion_tokens,messages,model,stream,stream_options,tool_choice,tools`）；三处 TS 断言（context / registry-parity / account-host）按 oracle 对齐并注明依据 | 已收敛（CI parity 门禁持续守护）    |
| M-2 | M    | modelRequests #0 `tool_choice`                     | ✅ **已修复（2026-10-14）**     | 同 M-1：AI SDK 在有工具时注入 `tool_choice:"auto"`；Rust 补发（仅 Chat 且有工具时）。验证同上                                                                                                                                                                                                                                                                                                                                                                                                     | 已收敛                              |
| M-3 | M    | modelRequests #1（标题请求）                       | 已批准                          | 标题请求形态：Node 非流式 / Rust 始终流式（`bench-escode-cli-node-rust.mjs` 已按请求 stream 应答并注释）                                                                                                                                                                                                                                                                                                                                                                                          | 统一标题请求形态时复核              |
| A-1 | A    | modelRequests #0 `messages`（5 条 vs 4 条）        | 已知环境差异（L3 覆盖）         | CLI 直跑无插件宿主 env：Node 自行把官方插件缓存装进 storage（`<storage>/cli/plugins/cache/…`，621 文件）并注入 skills/agents 提示；Rust 依赖宿主注入，故消息面为空。桌面路径由 Host 注入，须在阶段 2/3 的桌面 E2E 里验证插件面                                                                                                                                                                                                                                                                    | 桌面 L3 E2E 覆盖后从本条拆出        |
| S-1 | S    | storageFacts                                       | 记录待对拍                      | 存储布局天然不同（Node `ESCODE_STORAGE_DIR` 621 文件 vs Rust `--data-dir` 独立目录）；跨读（Rust 读 Node 库、反向亦然）演练在阶段 2 存储轮                                                                                                                                                                                                                                                                                                                                                        | 阶段 2 双 Host 交替写读演练         |
| F-1 | F    | workspaceFiles                                     | **一致（正面结论）**            | 场景结束后两侧 workspace 文件清单与内容 sha256 完全相同（0 差异）                                                                                                                                                                                                                                                                                                                                                                                                                                 | —                                   |

汇总（2026-10-14 收敛后）：frames 14（已批准 2、欠账 12、新 0）· storageProgress 1（批准）·
streams 1（open）· modelRequests 2（已批准，M-1/M-2 已修复）· workspaceFiles 0；两侧退出码均 0。

## 欠账与收紧顺序

1. ~~**M-1/M-2 模型请求字段名**~~：2026-10-14 已修复并验证（见上表）。
2. **T-3 mcpTelemetry**：消费方已查明=ARMS 埋点（观测面）；实现归阶段 2 P2（MCP 进程账本/遥测），本文件保留记账。
3. **C-2 memoryEnabled**：遥测事实与后续 ack 已收敛（见上表）；createSession ack 与请求位置差异已登记为已批准（含移除条件）。
4. **T-4 seq 编号/状态载体**：与阶段 2 投递层对齐一并处理（涉及投递状态机重构）。
5. **frames 欠账 12 条**：以上 1–4 收敛后重新对拍并 `--update-baseline` 清账；
   `--strict` 在欠账清零后接入 CI（当前 CI 门禁拦新差异）。

## 复现

```bash
node scripts/build-desktop-agent-cli.mjs                     # Node bundle
cargo build --release --locked --manifest-path apps/escode-cli-rust/Cargo.toml --bin escode-cli-rust
node scripts/diff-escode-cli-rust-parity.mjs                 # 判定 + evidence + 报告
node scripts/diff-escode-cli-rust-parity.mjs --strict        # 欠账/open 也判失败（清账后使用）
node scripts/diff-escode-cli-rust-parity.mjs --update-baseline  # 分类完成后重设欠账基线
```

CI：`rust-runtime-ci.yml` 的 bench job（三平台）在同一次运行里追加 parity 差分门禁并把报告写进 job summary。
**观察期**：基线指纹目前只在本机 Windows 验证过稳定（稳定复跑 3 次）；该 CI 步骤暂设 `continue-on-error`，
三平台首次全绿后去掉该开关转为阻塞（拦新差异）。
