# Rust Host 项目与任务组织态（W3）

> 状态：W3 已落地（2026-10-28）；缺口见 §5
> 关联：[upstream-v3.16.2-sync.md](upstream-v3.16.2-sync.md)、[source-server-remote-rust-host.md](source-server-remote-rust-host.md)、[rust-unified-host.md](rust-unified-host.md)
> Oracle：上游 ZCode v3.16.2 `packages/services/src/{project-storage,project,project-catalog,task-facts,task-groups,window-project-directory}`（只读，不导入 Node 实现）

## 1. 范围与边界

W3 把上游 Node Host 的项目/存储数据模型与组织态行为落到 Rust Host：

| 域                | 上游位置                         | Rust 落点                                             |
| ----------------- | -------------------------------- | ----------------------------------------------------- |
| 项目容器与 roots  | `project-storage/project-repository.ts` | `host-services/src/tasks_index/project_{read,write,misc}.rs` |
| 项目服务方法面    | `project/index.ts`               | `host-rpc/src/domains_project.rs`                     |
| 任务分组          | `task-groups/repository.ts`      | `host-services/src/task_groups/` + `host-rpc` channel |
| 自动归档          | `project/autoArchiveScheduler.ts`| `host-rpc/src/project_archive.rs`                     |
| 任务事实          | `task-facts/index.ts`            | 既有 `task_index_syncer` / `tasks_index::write`       |
| 项目/目录选择     | `project-catalog/*`              | `project.readDirectory` 组合 `file.readdir`           |

归属：

- **唯一写者**：Rust Host 的 `TasksIndex`（`tasks-index.sqlite`，含 `projects` / `project_roots` / `task_group_*` 表）。Renderer 不直接写库。
- **远程**：按 Q3，本轮远程 workspace 仍由窗口内 Node Host 承载；Rust Host 只服务本地 scope。`hostId != local` 的请求返回 `project.host-offline` / `project.cross-host-move`，不做静默降级。
- **Node 代码**：仅作 oracle，差分通过后按既有 M5 计划删除。

## 2. 数据模型

生成资产 `apps/escode-host/crates/host-services/src/tasks_index/schema.rs` 由
`scripts/generate-escode-host-tasks-schema.mjs` 从 oracle（`scripts/upstream-sync/project-storage-oracle/`）产出：

- tasks 库：`tasks_schema_migration` 账本 + 6 条迁移（含 `0004` 项目执行列、`0006_task_group_admissions`）。
- project 库：`schema_migration` 账本 + 3 条迁移（`0005_projects`、`0006_default_workspace_tasks`、`0007_initial_workspace_order`）。
- `TasksIndex::open` 同时校验两个账本与 DDL 哈希；账本不符即拒绝打开，不回退、不重建。

任务行新增列（由迁移加法写入）：`host_id`、`root_path`、`project_id`、`execution_directories`、`cli_created_at`、`cli_updated_at`、`handoff_pending`。
组织态表：`hosts`、`projects`、`project_roots`、`remote_project_entries`、`task_group_admissions`、`task_groups`、`task_group_members`、`task_group_view_node_orders`、`task_group_workspace_bootstraps`。

## 3. 方法面与状态时序

`project` channel（`ServiceChannels.Project`）：

| 方法 | 行为 | 失败 |
| ---- | ---- | ---- |
| `readDirectory({hostId?,path?,includeHidden?})` | 缺省 `home`；必须绝对路径；`path.normalize` 后委托 `file.readdir` | `project.absolute-path-required`、`project.directory-service-unavailable` |
| `readProjects({includeDeleted?})` | 项目列表 + 所属 Host 派生的 `primaryRootStatus`（只读，不触发归档） | 索引错误原样上抛 |
| `readTree({includeDeleted?,includeDeletedTasks?})` | `{projects,tasks,deletedTasks?}` + `primaryRootStatus` | 同上 |
| `saveProject(input)` | roots 绝对路径 + 存在且为目录 + 归一化/去重；主 root 必须属于 roots；名字去重追加 `(n)` | `project.absolute-path-required`、`project.directory-required`、`project.invalid-primary-root` |
| `deleteProject(id)` | 幂等软删，保留 roots 与任务归属 | — |
| `moveTask({task,projectId,addMissingRoot?})` | 跨 host 拒绝；缺 root 需显式 `addMissingRoot` 且目录存在；同事务写 `project_id` + `handoff_pending` | `project.root-confirmation-required`、`project.cross-host-move` |
| `reorderProjects(ids)` | 写事务内校验存在性与 host，写 position | `project.duplicate-order` |
| `prepareSession({projectId?,draftId?,reuseDraft?})` | 见 §3.1 | 见下 |
| `resolveSessionDirectories(task)` | 已有任务：root + 项目 roots 派生附加目录；未知 task 返回空附加目录 | `project.invalid-execution-directories` |

### 3.1 `prepareSession`

```
caller ──▶ Host project.prepareSession
             │ projectId? ──yes──▶ 读 tree(includeTasks:false) → 找项目
             │                       ├ 无 roots：mkdir(<workspaceDir>/<id>) → initializeProjectRoot（同事务空判断）
             │                       ├ 主 root 必须 available（missing/unavailable 直接拒绝）
             │                       └ 复读项目：期间被删 → project.not-found
             └ no ───────────────▶ workspaceDir/[draft-<uuid> | <ts>-<uuid>]
                                     ├ reuseDraft：目录必须已存在，不 mkdir 复活
                                     └ 否则 mkdir -p
```

- `draftId` 必须是 UUID；`draftId` 与 `projectId` 同时给、或 `reuseDraft` 无 `draftId` → `project.invalid-draft-target`。
- 同一进程内 `prepareSession` 串行化（`tokio::sync::Mutex`），并发同 `projectId` 只建一次 root。
- `workspaceDirectory` = `<dataBaseDir>/.escode/workspace/default`（与 `file.ensureConversationWorkspace` 同源）。
- 返回 `{workspacePath, additionalDirectories, projectId}`；本地 scope 不填 `workspaceIdentity`（Host 不推断 attachment 身份）。

### 3.2 自动归档

唯一触发者是 Host 定时扫描，**不挂在任何读/点击路径上**：

```
spawn ──▶ 首轮 scan ──▶ readPolicy(settings) ──▶ archiveInactive(cutoff, activeTasks)
                        └ null（关闭）→ 空操作        └ 有变更 → 广播项目/任务变更
        ──▶ every 5 min scan（运行中收到设置变更 → 合并为一次补扫）
```

- 策略键：`taskAutoArchiveEnabled`、`taskAutoArchiveOlderThanDays`（默认 30 天，1–365）；读取失败按关闭处理并 `warn`。
- 活动任务来自 syncer 已观察到的非终态会话（TS `activeTaskRefs`），不为扫描启动运行时。
- 旧版遗留（开关开启但无 `taskAutoArchiveResetVersion`）首次读取按关闭处理，与 TS `migrateTaskAutoArchiveReset` 同义，避免升级后突然归档历史会话。
- `archiveInactive` 只按 `cli_updated_at < cutoff`、且置顶/归档/删除/活动任务除外，写 `archived=1`。
- 设置变更通过 `setting.update` 后的补扫信号接入；不在 UI 读取时归档（历史 bug：点开即消失）。

### 3.3 任务分组

`task-groups` channel（`ServiceChannels.TaskGroups`）：`read` / `save` / `ungroup` / `move` / `admit`。

- `read` 只读快照，不写库、不登记。
- `admit` 由任务列表首次发现时调用：写 `task_group_admissions`，首次登记才分配顶层位置；`cron` 任务首次确认类型后以 `system_kind='cron'` 入默认组。
- `move` 校验来源组与目标组，`sourceGroupId` 不匹配 → `task-group.stale-source`；锚点不存在 → `task-group.stale-anchor`。
- 已删除的闲时任务域（`offPeak`）不进入分组模型（Q4）。
- 所有写操作在单个 `BEGIN IMMEDIATE` 事务内完成，无跨进程改写任务事实。

### 3.4 任务事实

- 任务行的 CLI 事实（`cli_title`、`cli_created_at`、`cli_updated_at`、`cli_last_outcome`、`cli_deleted`）仍由 `task_index_syncer` / `TaskIndexRepo` 写面维护，Host 时钟不推导会话事实。
- `task-facts.repairSubagentTaskIndex` 的冷恢复修复复用 `tasks_index` 的删除/孤儿标记写面；不创建运行时。
- 项目归属（`project_id`、`root_path`、`handoff_pending`）只在 `moveTask` / `reserveCreatedTask` 事务内写；读取路径不补齐。

## 4. 验收

| 场景 | 断言 |
| ---- | ---- |
| 两个并发 `prepareSession({projectId})` | 同一 workspacePath；`project_roots` 只有一行 |
| `prepareSession({draftId})` 重复调用 | 同一 `draft-<id>` 目录；无 draftId 的调用分配独立目录 |
| `draftId="../../escape"` | 拒绝，且不创建目录 |
| `reuseDraft` 目录被移走 | `project.directory-required`，不重建 |
| 项目在目录校验期间被删 | `project.not-found`，不返回旧归属 |
| `saveProject` 相对路径 / 不存在目录 | 各自错误码，且不写库 |
| `moveTask` 缺 root 且未确认 | `project.root-confirmation-required`；确认后原子加 root + 移动 |
| `readProjects` / `readTree` | 返回 `primaryRootStatus`；多次读取不改变 `archived` |
| 自动归档 | 5 分钟 tick + 首轮扫描；读取/点击不归档；设置关闭时空操作 |
| task-groups | `admit` 幂等；`move` 来源/锚点过期拒绝；`ungroup` 保留组织墓碑 |
| 契约 | `pnpm check:escode-host`（含 inventory 对账）、`cargo test -p escode-host-services -p escode-host-rpc -p escode-host-proto` |

未执行或环境受限的检查在同步 spec 的波次记录中如实标注。

## 5. 已知缺口

1. `task-facts.repairSubagentTaskIndex` 的启动期全量清扫未单独落地。syncer 已阻止
   `subagent_child` 重新写入主列表，但旧索引中残留的 child 行没有启动清扫；需要 Agent
   `listSessions(existing-only)` 的批查通道，计划随 W4 的会话 E2E 一起补。
2. Source Server / 远程 scope 不在 Rust Host：按 Q3 由窗口内 Node Host 承载；后续统一方案见
   [source-server-remote-rust-host.md](source-server-remote-rust-host.md)。
3. 旧 `escode-task` 分组方法（`createTaskGroup` / `applyGroupedTaskViewOrder`）仍是 UI 的过渡面，
   `task-groups` channel 已可用；W4 迁移 UI 后删除旧面。

