# 上游 ZCode v3.16.2 同步计划

> 状态：决策已确认，W0 清单已生成 · 2026-10-28
> 关联：[source-server-remote-rust-host.md](source-server-remote-rust-host.md)、[rust-tool-search.md](rust-tool-search.md)、[w2-port-rust-review.md](../upstream-sync/w2-port-rust-review.md)、[rust-workflow-blocked-reminder.md](rust-workflow-blocked-reminder.md)、[escode-rebrand.md](escode-rebrand.md)、[rust-v3.15.1-drift.md](rust-v3.15.1-drift.md)、[rust-parity-remaining.md](rust-parity-remaining.md)、[rust-unified-host.md](rust-unified-host.md)
> 清单：`docs/upstream-sync/v3.16.2-manifest.tsv`（生成脚本 `scripts/upstream-sync/build-manifest.mjs`）

## 1. 基线与规模

| 项目                      | 值                                                    |
| ------------------------- | ----------------------------------------------------- |
| 上游仓库                  | `https://github.com/zai-org/ZCode`                    |
| 上游目标                  | `main @ 3c5b2276`，版本 **3.16.2**（2026-10-20 快照） |
| 合并基线                  | 上游 `v3.15.1`，提交 `aac47556`（当前 `dev` 的祖先）  |
| 当前产品版本              | 3.15.2，同步完成后记为 **3.16.2-escode.1**            |
| 增量文件                  | **4,623**（修改 2,535 / 新增 1,737 / 删除 351）       |
| 源码 / 测试 / 文档 / 工具 | 1,538 / 2,608 / 430 / 47                              |
| 上游 Rust 改动            | **0**                                                 |

上游只有两个提交：`0a1fc56d`、`3c5b2276`，提交信息均为 `chore: sync ZCode source`。它们是整仓快照，**不能按单个 PR 或工单 cherry-pick**；本计划以 diff 清单为唯一事实来源。

## 2. 已确认决策

| 编号 | 决策                                                                                                                                                                                                        |
| ---- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Q1   | 按全量清单分波推进；对外版本 `3.16.2-escode.1`                                                                                                                                                              |
| Q2   | Node CLI / runtime / Host 代码只作只读 oracle；行为移植到 Rust，差分验收后再按既有 M5 计划删除                                                                                                              |
| Q3   | 本轮只跟进 Source Server 的协议/兼容字段与只读展示；远程 workspace 仍走现有 Node Host。后续统一到 Source Server 并落到 Rust Host，见 [source-server-remote-rust-host.md](source-server-remote-rust-host.md) |
| Q4   | 已删除功能域硬排除；共享文件按“剔除片段、保留其余”处理                                                                                                                                                      |
| Q5   | manifest 驱动的逐文件三方移植，不做 `git merge`                                                                                                                                                             |
| Q6   | 顺序：W0 清单 → W1 共享协议/数据模型 → W2 Rust runtime → W3 Rust Host/远程 → W4 UI/桌面 → W5 测试/文档                                                                                                      |
| Q7   | 共享/UI 的 TS 测试按品牌映射导入；Node runtime 行为改成 Rust 差分语料，不直接导入 Node 单测                                                                                                                 |

## 3. 清单分布

生成命令：

```sh
node scripts/upstream-sync/build-manifest.mjs
```

| 处置                | 文件数 | 含义                                                     |
| ------------------- | -----: | -------------------------------------------------------- |
| `import-test`       |  1,477 | 共享/UI 测试，随所属波次导入                             |
| `port-shared`       |    731 | shared / UI / desktop / server 的 TS 代码，三方移植      |
| `convert-rust-diff` |    581 | Node/Host 行为测试，改写为 Rust 差分语料                 |
| `exclude-deleted`   |    504 | 已删除功能域，整文件排除                                 |
| `manual-review`     |    444 | 需逐项确认（改名/删除、保留的 telemetry 事实、根级文件） |
| `adopt-docs`        |    374 | 文档，按品牌映射摄入                                     |
| `oracle-only`       |    258 | Node 内部结构改动，仅作 oracle                           |
| `port-rust-host`    |    121 | Node Host / 数据模型行为，移植到 Rust Host               |
| `port-rust`         |     91 | Node runtime 可观察行为，移植到 Rust runtime             |
| `adopt-tooling`     |     42 | 脚本、配置、治理工具                                     |

| 波次 | 文件数 | 范围                                                                      |
| ---- | -----: | ------------------------------------------------------------------------- |
| W1   |    145 | V4 协议、身份、SQLite 迁移（shared）                                      |
| W2   |    774 | Rust runtime：tool search / deferred call、session、model runner、schema  |
| W3   |    410 | Rust Host：项目/存储数据模型、Source Server 协议/只读面、database startup |
| W4   |  2,334 | UI / desktop：composer、项目侧栏、settings、ToolCallBlocks、gen-ui        |
| W5   |    456 | 文档、工具链、manual-review                                               |
| —    |    504 | 排除，不进入波次                                                          |

## 4. 删除域与排除规则

用户已确认不恢复以下功能，命中即整文件排除（共 504 个）：

- Web / 手机远控：`packages/web`、`packages/android`、web-remote-control / relay v2、mobile-ui
- 会话分享：conversation-share
- Bots 与 channel reply
- off-peak 闲时任务
- 遥测上报（保留内部 `v4/telemetry/event` 会话活动事实；这些文件若 fork 仍存在则转 `manual-review`）
- 问题上报 feedback
- Z.ai 账号 / 套餐族：`services/oauth`、`coding-plan-subscription`、`bigmodel`、`cloud-content`、`marketing-touch`

排除判定不是纯关键词：

1. 命中删除域、且映射后的 fork 路径不存在 → `exclude-deleted`。
2. 命中删除域、但 fork 仍有同名源码/测试（例如内部 telemetry facts）→ `manual-review`，避免误删仍在用的能力。
3. 命中删除域的文档 → 一律排除，避免把已下线功能文档重新带回。

共享文件里夹带的单个片段（例如协议里的 `offPeakControl` / `offPeakLifecycle`、`telemetry` 字段）在对应波次内剔除，不整文件丢弃。

## 5. 同步机制

### 5.1 逐文件三方移植

对清单中每个 `port-*` / `import-test` 文件：

1. `base` = `git show aac47556:<upstream_path>`（上游 3.15.1）
2. `theirs` = `git show refs/remotes/upstream/main:<upstream_path>`（上游 3.16.2）
3. `ours` = 当前分支的 `fork_path`
4. 对 `base` / `theirs` 施加品牌映射后，执行 `git merge-file`，逐文件落盘。
5. 逐文件结果写入 `docs/upstream-sync/port-report.tsv`，冲突块保留在 `docs/upstream-sync/conflicts/*.merge`；`resolve-conflicts.mjs` 只自动丢弃删除域片段，其余语义冲突人工处理。

路径映射按两个候选匹配，命中即用：前缀映射（`apps/zcode-cli` → `apps/escode-cli`、`packages/zcode-*` → `packages/escode-*`）和全路径品牌映射（`zcode` → `escode`）。内容映射遵循 [escode-rebrand.md](escode-rebrand.md) 的保留清单；`third-party/upstream-zcode/`、CDN 前缀、`zcode.z.ai` 等服务端地址、`x-zcode-*` 请求头、官方插件市场 id 不替换。

### 5.2 Node 与 Rust 的分工

- `oracle-only`：不移植，不删除；用于对照行为和生成差分语料。
- `port-rust` / `port-rust-host`：先写 spec/差分场景，再改 Rust；Node 行为是 oracle。
- `convert-rust-diff`：把 Node 单测改成“同一 fixture 下 Node/Rust 输出一致”的差分测试，不导入 Node 内部 mock。
- `port-shared`：直接三方移植 TS 代码，保持公开入口和架构边界。
- `import-test`：按品牌映射导入，跟随所属波次。

协议改动与 Rust 协议 crate 必须在同一改动集内落地：`pnpm test:escode-cli-rust` 的漂移检查以 TS 契约为 oracle，只改 `packages/shared/src/zcode-protocol*` 会让检查变红。因此 W1 的 V4 schema 变更与 W2 的 `crates/protocol` 更新按同一 PR 提交，或先更新 Rust 侧生成资产再切换 TS 消费方。

### 5.3 清单是唯一事实来源

- 清单由脚本生成，不手改；处置需要调整时，改脚本规则并重新生成。
- 每个波次开始前先看对应 `upstream_path` / `disposition` / `fork_state`，再动代码。
- `fork_state` 为 `missing` 的修改项表示 fork 已删同名文件，必须按功能域判断，不能直接新建。

## 6. 波次与验收

每波开始前更新本 spec 的进度表；每波结束记录命令、结果和未执行原因。

| 波次 | 交付物                                                                       | 验收                                                                                                                                                     |
| ---- | ---------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| W0   | 清单、生成脚本、本 spec、Source Server 决策 spec                             | 清单统计与上游 diff 一致；脚本可重复生成                                                                                                                 |
| W1   | V4 协议新增面、身份、SQLite 迁移、项目/存储模型                              | `pnpm typecheck`、`pnpm lint`、`pnpm fmt:check`、`pnpm architecture:check --changed`、`pnpm test:unit:affected`、`pnpm test:escode-cli-rust`（协议漂移） |
| W2   | Rust runtime 行为对齐（tool search/deferred、session、model runner、schema） | 上述 TS 检查 + `pnpm check:escode-cli-rust`、`pnpm test:escode-cli-rust`；新增差分语料                                                                   |
| W3   | Rust Host 协议/只读面、database startup、远端 Server                         | 上述检查 + `pnpm check:escode-host`、`pnpm test:escode-host`                                                                                             |
| W4   | Composer、项目侧栏、settings、ToolCallBlocks、gen-ui                         | 上述 TS 检查 + 目标桌面 E2E（`pnpm test:e2e` 或包内定向用例）                                                                                            |
| W5   | 文档、治理工具、manual-review 收口                                           | `pnpm fmt:check`、`pnpm lint`；文档链接检查                                                                                                              |

Rust 差分语料必须能复现，且不依赖真实供应商、网络或时间；使用固定 fixture 和虚拟时间。未执行或环境受限的检查在波次记录中如实标注，不写成通过。

## 7. 风险

- **排除误伤**：504 个排除文件里可能夹带仍在使用的事实类型；`manual-review` 已兜住同名文件，波次内仍需逐项确认。
- **W4 体量**：2,334 个文件，其中大量是 UI 测试与契约样板；按功能域拆成多个 PR，不一次性提交。
- **Source Server 边界**：本轮只跟协议，远程仍由 Node Host 承载；任何“顺手把远程迁到 Rust Host”的改动都属于下一阶段，需另立 spec 和验收。
- **Node oracle 消失**：Rust 差分验收前不得删除 Node 代码；删除后差分语料必须已固化。
- **快照式上游**：没有逐 PR 历史，无法按工单回滚单个上游改动；回滚以波次为单位。

## 8. W1 完成记录（2026-10-28）

范围调整：`project-storage` / `project-catalog` / `task-groups` / `task-facts` / `window-project-directory` 的 TS 实现是 Rust Host `tasks_index` schema 生成器的输入，依赖 Host 服务与 Source Server；按依赖关系并入 W3。W1 只保留 shared 协议、身份与 `sqlite-migrations`。

已落盘：

- V4 协议：`runtimeInitialization`、`sessionsIndexReconcile`、`workflowBlocked`、`historyFirst` / `historyReadyMs`、`sourceServerId` 身份前缀、`highspeed_selection_unavailable`。
- 新模块 `packages/shared/src/sqlite-migrations`（备份、恢复、表快照、迁移 runner、legacy）。
- 品牌映射与三方移植脚本：`port-wave.mjs`、`resolve-conflicts.mjs`；26 个冲突已收口（12 个自动、14 个人工）。
- 删除域片段已剔除：`offPeak` 执行参数、`offPeakControl` / `offPeakLifecycle` 方法。

刻意的跨波次兼容：

- `isDynamicWorkflowOffered` 保留为 `readDynamicWorkflowOfferedMode` 的 deprecated 包装，等 W4 迁移 UI。
- `packages/client/src/remoteServiceAccess.ts`、`messageport.ts`、`websocket.ts` 回退到 HEAD；上游新增的 Host 服务类型随 W3 一起落。
- `packages/shared/src/{index,protocol,channels,validation,validationAppSettings,server-remote,escode-protocol/index}.ts` 回退到 HEAD；这些 barrel 同时引用 Source Server、Web 远控与分享模块，必须等 W3/W4 分域处理。
- `packages/shared/test/appSettings.test.ts` 回退到 HEAD；上游版本断言 Source Server / 远程边界设置，属 W3。

验证：

| 检查                                                                        | 结果                                                                     |
| --------------------------------------------------------------------------- | ------------------------------------------------------------------------ |
| `pnpm typecheck`                                                            | 通过（0 error）                                                          |
| 定向 `vitest`（shared 协议、sqlite-migrations、identity、dynamic workflow） | 113 passed                                                               |
| Rust 漂移：command-schema / prompt / tool-schemas / permission-matrix       | 全部通过                                                                 |
| 改动文件 `oxlint`                                                           | 0 error                                                                  |
| 全量 `pnpm lint`                                                            | 1 error 来自未跟踪的 `packages/desktop/index.js`（构建产物，非本次改动） |
| 全量 `pnpm fmt:check`                                                       | 既有 166 个生成文件失败；本次改动文件格式检查通过                        |

## 9. W3 完成记录（2026-10-28）

范围：Rust Host 的项目/存储数据模型与组织态。远程 workspace 按 Q3 仍由窗口内 Node Host 承载，
本轮不把 Source Server 落到 Rust Host（见 [source-server-remote-rust-host.md](source-server-remote-rust-host.md)）。
详见 [rust-project-storage.md](rust-project-storage.md)。

已落盘：

- tasks-index 生成资产：6 条 tasks 迁移 + 3 条 project 迁移；`TasksIndex::open` 同时校验
  `tasks_schema_migration` 与 `schema_migration` 两个账本与 DDL 哈希。
- `project` channel 补全上游 `IProjectService`：`readDirectory`、`prepareSession` 以及
  `saveProject` 的绝对路径/目录存在性校验、`moveTask` 的 `addMissingRoot` 目录复检、
  `readProjects`/`readTree` 每次读取派生的 `primaryRootStatus`。
- `initializeProjectRoot`：空项目才在写事务内写入主 root。
- 自动归档：Host 定时扫描（首轮 + 5 分钟）+ 自动归档设置变更补扫；活动任务取 syncer 观察到的
  非终态会话（`activeTaskRefs`），读取/点击路径不触发归档。设置默认保留 30 天，并补上
  `taskAutoArchiveResetVersion` 一次性重置迁移（旧的“已开启但未执行”设置升级时先关闭）。
- `task-groups` channel：`read` / `save` / `ungroup` / `move` / `admit`，与旧 `escode-task`
  分组方法共用同一批 `task_group_*` 表；闲时任务域（`offPeak`）不进入模型。
- TS 契约：`project-storage` / `project` / `task-groups`，`ServiceChannels.Project` /
  `TaskGroups`；Host 服务面清单从 43 增至 45 channel、509 成员。

验证：

| 检查                                         | 结果                                  |
| -------------------------------------------- | ------------------------------------- |
| `pnpm check:escode-host`                     | 通过（boundaries / 生成资产 / clippy） |
| `cargo test -p escode-host-services -p escode-host-proto` | 550 + 5 passed              |
| `cargo test -p escode-host-rpc --tests`      | 161 lib + 全部集成测试 passed          |
| `pnpm typecheck`                             | 通过（0 error）                       |
| `pnpm lint`                                  | 0 error（191 条既有 warning）         |
| `pnpm architecture:check --changed`          | 通过（0 violation）                   |

刻意的跨波次差异（如实记录）：

- `task-facts.repairSubagentTaskIndex` 的冷恢复全量清扫尚未单独落地：Rust syncer 已阻止
  `subagent_child` 重新写入主列表（`task_index_syncer/apply_writes.rs`），但旧索引中已存在的
  child 行没有启动期清扫。待 W4/W5 在有 E2E 场景时补。
- Source Server：shared 协议字段（`sourceServerId` 身份前缀、controller schema）已在 W1 就位；
  Rust Host 不注册 source-server scope（Q3/Q4），远程仍由 Node Host 提供。
- 旧 `escode-task` 分组方法（`createTaskGroup` 等）保留为过渡面，W4 迁移 UI 后由
  `task-groups` channel 取代。

## 10. W4 进度

### W4-1 ToolCallBlocks（2026-10-28）

`generator --prefix` 加入 `port-wave.mjs` 后按功能域分批；本批 26 个文件：

- 21 个 `port-shared`（`ToolCallBlocks.tsx` 与 `ToolCallBlocks/**`）+ 1 个 `import-test`（`toolCallBlocks.test.ts`），
  其中 18 合并、4 新建、1 冲突。
- 依赖闭包：`hooks/useToolPresentationHostCapabilities.ts`、`chat-input-toolbar/thoughtLevelLabels.ts`、
  `lib/nodeReplToolDisplay.ts`（`NodeReplDisplayImage.src`）。
- 测试冲突取本 fork 的两条 MCP 渲染断言（上游与新展示无关），合并后 38 条断言全部通过。
- `PermissionDialog` 的合成上下文补 `nestedToolRenderer`：权限预览 `childToolCalls` 恒空，用显式空渲染器，
  不把完整 `ToolCallBlock` 依赖带进权限对话框。
- `agentToolCallBlock.test.ts` 的配置颜色用例改用 `ToolPresentationCapabilitiesProvider` 注入
  `agentColor`，与新的“宿主能力注入、renderer 不读 store”边界一致；其余两个分组测试补
  `nestedToolRenderer`。

验证：`pnpm typecheck` 通过；`vitest` 46 个工具卡片测试文件共 746 passed；改动文件 `oxlint` 0 error；
`oxfmt --check` 通过。

### W4-2 Composer 工具栏（2026-10-28）

`--prefix packages/ui/src/chat-input-toolbar`，10 个文件（5 新建 / 4 合并 / 1 保留本地），另加 6 个依赖文件：

- Composer 菜单：`ComposerModelMenu`、`ComposerProviderMenu`、`ComposerEffortHeading`、
  `ComposerTriggerTransition`、`composer-thought-slider.css`；合并 `contextUsage`、`display`、
  `ThoughtLevelCycleControl`、`thoughtLevelOptions`。
- 依赖闭包：`ControlShortcutBadge`、`lib/composerPlanGroups`、`components/ui/dropdown-menu`、
  `ModelConfigSelect`（`ModelSelectGroup.logo` 等新增字段）、`lib/teamPlanDisplayName`
  （上游未改但 fork 从未有过的同基线文件）、`test/composerPlanGroups.test.ts`。
- 冲突：`contextUsage` 采用 fork 的 `@escode/shared/test-ids` 导入约定，上游新增的
  `CodingPlanResetType` 属已删除的 coding-plan 域且在该文件未被使用，不引入。
- 全量 UI 回归暴露并修复：上游新增 `web-fetch` family 后，`WebFetch` 权限请求不再落
  `search`；移植 `PermissionDialog` 并补 `web-fetch` 块（`WebFetchToolCallBlock`）与
  `ToolPresentationCapabilitiesProvider` 注入。手机 Web 远控相关的 `isWebRemoteControl`
  视口逻辑按 Q4 不引入。

**网络与工具链**：本环境 github.com 的 smart-HTTP 不可达，本地 partial clone 只有 78/2,334 个
W4 blob。`port-wave.mjs` 现在对缺失 blob 回退到 jsDelivr / `raw.githubusercontent.com`
（按 commit sha），并把结果缓存到 `.git/upstream-sync-raw`（预取后可离线重跑）；对
“上游新增但本地已存在”的文件记录 `kept-local-new` 而不误判为冲突。

验证：`pnpm typecheck` 通过；相关 10 个测试文件 **111 passed**；全量 `packages/ui/test`
**7,584 passed / 1 failed**——失败项是 `rootUpdateReadyOverlay.test.ts > 独立设置页不会挂载更新按钮`，
原因是 `Root` 的 lazy `SettingsPage` 在 SSR 下只能渲染 `Suspense` fallback，与本批改动的模块
没有依赖关系，已记录为既有环境性失败。改动文件 `oxlint` 0 error、`oxfmt --check` 通过。

### W4-3 共享组件（2026-10-28）

`--prefix packages/ui/src/components`，16 个文件全部干净合并，另加依赖 `ControlHintTooltip`（新增
`SidebarNativeTooltip`）：

- `ai-elements/{chat-loading,image-preview-dialog,reasoning}`、`PluginIcon`、
  `ui/{collapsible,dialog,dropdown-menu,hover-card,kbd,resizable,toast,tooltip}`、
  `workflow-run-line/WorkflowRunRail`、`workflow-timeline/{subagent-model-label,WorkflowCardChrome,WorkflowRunDigest}`。
- 测试按上游更新：`controlHintTooltip`、`pluginStoreAvatar`、`v4ConversationStatusPanel`、
  `workflowHoleSurfaces`（`v4ConversationStatusPanel` 仅用例名对齐）；`collapsible.test.ts`
  上游已删除（断言的是旧实现细节），本 fork 同步删除。

验证：`pnpm typecheck` 通过；全量 `packages/ui/test` **7,574 passed / 1 failed / 7 skipped**，
失败项仍是 §W4-2 记录的 `rootUpdateReadyOverlay` 既有环境性失败；改动文件 `oxlint` 0 error、
`oxfmt --check` 通过。

### W4-4 Store 批次（暂缓，2026-10-28）

尝试 `--prefix packages/ui/src/store`（19 个修改 + 2 个新增）。逐文件合并成功，但类型检查暴露
它与尚未迁移的 grouped-draft / authority 界面强耦合：`WorkspaceESCodeUIState` 需要
`groupedDraftTask`、`draftSessionId`、`timelineBottomRequest`、`promotedGroupedDraftTaskByTaskId`，
`ESCodeSessionStoreState` 需要 `clearGroupedDraftTask` / `promoteGroupedDraftTask`，同时
`App.tsx`、`SessionPane.tsx`、`useGroupedTaskView`、`workspaceTaskListOptimisticOverlay` 仍是旧契约。
本批已整体回滚，保持仓库可编译；后续与这些消费方合并成同一批迁移。

## 11. 当前状态

- [x] 抓取上游 `refs/remotes/upstream/main`（`3c5b2276`）
- [x] 生成 `docs/upstream-sync/v3.16.2-manifest.tsv` 与 summary
- [x] 记录 Q1–Q7 决策与 Q3 后续路线
- [x] W1 共享协议与 SQLite 迁移（145 文件）
- [x] W2 ToolSearch / DeferredToolCall（local + native，差分语料）
- [x] W2 workflow blocked reminder（15/30 分钟空闲提醒，见 [rust-workflow-blocked-reminder.md](rust-workflow-blocked-reminder.md)）
- [x] W2 tool result settlement / modelCallWrapper（见 [w2-port-rust-review.md](../upstream-sync/w2-port-rust-review.md)）
- [x] W2 tool search 差分：上游 TS oracle + Rust fixture 实时比对（19 cases）+ 冻结语料 29 cases（Node CLI 本 fork 无该功能）
- [x] W3 Rust Host 项目/存储数据模型、自动归档、任务分组（见 §9）
- [ ] W4 UI / 桌面（ToolCallBlocks、Composer 工具栏已迁，见 §10）
- [ ] W5 文档 / 测试 / 治理

W0 只新增清单、脚本和文档，没有修改产品源码。
