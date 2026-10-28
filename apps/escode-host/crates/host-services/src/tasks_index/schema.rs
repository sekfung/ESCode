//! tasks-index 的迁移账本与物理 DDL，由 `scripts/generate-escode-host-tasks-schema.mjs` 从 TS 生成，勿手改。
//!
//! 期望账本来自 project-storage oracle 在临时库上跑完「tasks 基础迁移 + project-storage 迁移」后的
//! `tasks_schema_migration` 与 `schema_migration`；DDL 来自同一库的 `sqlite_master`。
//! TS 迁移定义变化 → 本文件变化 → `--check` 在 CI 失败。

/// 生成器版本（提取方式变化时递增）。
pub const SCHEMA_VERSION: u32 = 2;

/// 期望的迁移账本 `(id, checksum)`（按 id 升序）。
/// 生成文本长度固定，`#[rustfmt::skip]` 保证生成结果与 rustfmt 无关（否则 `--check` 会被格式化差异干扰）。
#[rustfmt::skip]
pub const EXPECTED_MIGRATIONS: &[(&str, &str)] = &[
    ("0001_adopt_task_schema", "3e8337b015d94b05dd31a6003f3acc649e821794cfa288bc0af3022698bd4d17"),
    ("0002_provider_selection", "7244ef7c351f8d02750ab1953fff09f493a71befbf1b6e2d4bab726b0c6b48fc"),
    ("0003_official_glm_selection", "8987adb50ae412a46c294141c1af89ccfc252f22d41351bdf4c7528f56edc8b4"),
    ("0004_drop_off_peak", "4d2550dfd13c3f532e973ab6e079e89f4effbf4af77775a0de5368f98c8e717a"),
    ("0005_project_execution_scopes", "159aee6e6d5854520f2838099581948c421ab19ac7a996589dc09b9e617f3de3"),
    ("0006_task_group_admissions", "0af559d04bb3f1a1dab12d6533d59292d232b09ea3ef3ef6ca3c9373bd663019"),
];

/// `EXPECTED_MIGRATIONS` 的 sha256（`\t`/`\n` 连接的规范文本，加载期自校验）。
#[rustfmt::skip]
pub const EXPECTATIONS_HASH: &str = "3b72d8c833f035399ee10e090605dc014992b6000270834a602e99bf8647c483";

/// project-storage 的迁移账本 `(id, checksum)`（`schema_migration`，按 id 升序）。
#[rustfmt::skip]
pub const EXPECTED_PROJECT_MIGRATIONS: &[(&str, &str)] = &[
    ("0005_projects", "ac00a61c7c45a5ceb3814b1487901372c5580eb6eb8d31bde5bcdfbd1ed0f0da"),
    ("0006_default_workspace_tasks", "0a29c1c943e4c794b2c3bec3e1a776fa3e182d92c342185d3bc9edc911c8e5c3"),
    ("0007_initial_workspace_order", "cac45e440b4b54f588d7611bd1512232e18dda3c87144949568e45251edbcf1b"),
];

/// `EXPECTED_PROJECT_MIGRATIONS` 的 sha256。
#[rustfmt::skip]
pub const PROJECT_EXPECTATIONS_HASH: &str = "3ad84cbee6b734a111518436d1a4b124cc18430f42caa0c640dd8e8cd4c95eb1";

/// 迁移后的物理 DDL（`sqlite_master.sql` 原样，先表后索引；测试夹具与后续切片建库用）。
#[rustfmt::skip]
pub const MIGRATED_DDL: &[&str] = &[
    "CREATE TABLE automation_runs ( run_id TEXT PRIMARY KEY, automation_id TEXT NOT NULL, workspace_key TEXT NOT NULL, scheduled_at INTEGER, trigger TEXT NOT NULL DEFAULT 'schedule', model_selection TEXT, dispatch_status TEXT NOT NULL DEFAULT 'claimed', outcome TEXT, session_id TEXT, error TEXT, attempts INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL )",
    "CREATE TABLE automations ( automation_id TEXT PRIMARY KEY, title TEXT NOT NULL DEFAULT '', cron_expr TEXT NOT NULL, prompt TEXT NOT NULL, model TEXT, provider TEXT, mode TEXT, thought_level TEXT, model_selection TEXT, workspace_key TEXT NOT NULL, workspace_path TEXT NOT NULL, workspace_identity TEXT, target_task_id TEXT, bot_delivery_target TEXT, location_kind TEXT NOT NULL DEFAULT 'local', recurring INTEGER NOT NULL DEFAULT 1, max_runs INTEGER, end_at INTEGER, schedule_rule TEXT, schedule_edited_by_user INTEGER NOT NULL DEFAULT 0, run_count INTEGER NOT NULL DEFAULT 0, scheduled_run_count INTEGER NOT NULL DEFAULT 0, enabled INTEGER NOT NULL DEFAULT 1, lifecycle_status TEXT NOT NULL DEFAULT 'active', next_run_at INTEGER, last_run_at INTEGER, running INTEGER NOT NULL DEFAULT 0, claimed_at INTEGER, dispatch_status TEXT NOT NULL DEFAULT 'idle', dispatch_attempts INTEGER NOT NULL DEFAULT 0, retry_at INTEGER, last_error TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL , project_id TEXT, host_id TEXT, additional_directories TEXT)",
    "CREATE TABLE hosts ( host_id TEXT PRIMARY KEY, kind TEXT NOT NULL, target_json TEXT, membership_id TEXT, position INTEGER NOT NULL DEFAULT 0, last_connection_status TEXT, last_connection_error TEXT, last_opened_at INTEGER)",
    "CREATE TABLE project_roots ( project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE, root_path TEXT NOT NULL, position INTEGER NOT NULL, is_primary INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(project_id,root_path))",
    "CREATE TABLE projects ( project_id TEXT PRIMARY KEY, host_id TEXT NOT NULL REFERENCES hosts(host_id), name TEXT NOT NULL, name_overridden INTEGER NOT NULL DEFAULT 0, position INTEGER NOT NULL, pinned INTEGER NOT NULL DEFAULT 0, pinned_at INTEGER, deleted INTEGER NOT NULL DEFAULT 0, deleted_at INTEGER, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL)",
    "CREATE TABLE remote_project_entries ( entry_id TEXT PRIMARY KEY, host_id TEXT NOT NULL REFERENCES hosts(host_id) ON DELETE CASCADE, remote_project_id TEXT, cached_name TEXT NOT NULL, cached_roots_json TEXT NOT NULL DEFAULT '[]', legacy_workspace_json TEXT, last_synced_at INTEGER, initial_position INTEGER, UNIQUE(host_id,remote_project_id))",
    "CREATE TABLE \"schema_migration\" (id text primary key, checksum text not null, app_version text, \"applied_at\" integer not null)",
    "CREATE TABLE task_group_admissions ( workspace_key TEXT NOT NULL, task_id TEXT NOT NULL, system_kind TEXT, user_organized INTEGER NOT NULL DEFAULT 0, creation_id TEXT, PRIMARY KEY (workspace_key, task_id) )",
    "CREATE TABLE task_group_members ( group_id TEXT NOT NULL, workspace_key TEXT NOT NULL, workspace_path TEXT NOT NULL, workspace_identity TEXT, task_id TEXT NOT NULL, sort_order INTEGER, added_at INTEGER NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, PRIMARY KEY (workspace_key, task_id), FOREIGN KEY (group_id) REFERENCES task_groups(group_id) ON DELETE CASCADE )",
    "CREATE TABLE task_group_view_node_orders ( node_type TEXT NOT NULL, node_key TEXT NOT NULL, sort_order INTEGER NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, PRIMARY KEY (node_type, node_key) )",
    "CREATE TABLE task_group_workspace_bootstraps ( workspace_key TEXT PRIMARY KEY, group_id TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL )",
    "CREATE TABLE task_groups ( group_id TEXT PRIMARY KEY, title TEXT NOT NULL, color TEXT NOT NULL DEFAULT 'gray', created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL )",
    "CREATE TABLE tasks ( workspace_key TEXT NOT NULL, workspace_path TEXT NOT NULL, workspace_identity TEXT, task_id TEXT NOT NULL, title TEXT NOT NULL DEFAULT '', task_status TEXT, provider TEXT, mode TEXT NOT NULL DEFAULT 'build', model TEXT, migration_source TEXT, forked_from_task_id TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, unread_at INTEGER, last_unread_at INTEGER NOT NULL DEFAULT 0, pinned INTEGER NOT NULL DEFAULT 0, archived INTEGER NOT NULL DEFAULT 0, deleted INTEGER NOT NULL DEFAULT 0, title_overridden INTEGER NOT NULL DEFAULT 0, meta_json TEXT NOT NULL DEFAULT '{}', searchable_text TEXT NOT NULL DEFAULT '', cron_automation_id TEXT, host_id TEXT, root_path TEXT, project_id TEXT, execution_directories TEXT, sort_order INTEGER, cli_title TEXT, handoff_pending INTEGER NOT NULL DEFAULT 0, cli_title_source TEXT, cli_created_at INTEGER, cli_updated_at INTEGER, cli_last_outcome TEXT, cli_deleted INTEGER, PRIMARY KEY (workspace_key, task_id) )",
    "CREATE TABLE tasks_schema_migration ( id TEXT PRIMARY KEY, checksum TEXT NOT NULL, time_applied INTEGER NOT NULL )",
    "CREATE INDEX idx_automation_runs_by_automation ON automation_runs (automation_id, created_at DESC)",
    "CREATE INDEX idx_automations_due ON automations (enabled, next_run_at)",
    "CREATE INDEX idx_automations_retry ON automations (enabled, retry_at)",
    "CREATE INDEX idx_automations_target_task ON automations(target_task_id) WHERE target_task_id IS NOT NULL",
    "CREATE INDEX idx_automations_workspace ON automations (workspace_key)",
    "CREATE UNIQUE INDEX idx_projects_host_name ON projects(host_id,name) WHERE deleted=0",
    "CREATE INDEX idx_task_group_members_group_order ON task_group_members (group_id, sort_order, added_at)",
    "CREATE INDEX idx_task_group_view_node_orders_order ON task_group_view_node_orders (sort_order, created_at)",
    "CREATE INDEX idx_tasks_cron_automation ON tasks(cron_automation_id, updated_at DESC) WHERE cron_automation_id IS NOT NULL AND deleted=0",
    "CREATE UNIQUE INDEX idx_tasks_host_task ON tasks(host_id,task_id)",
    "CREATE INDEX idx_tasks_project ON tasks(project_id,archived,cli_updated_at DESC) WHERE deleted=0",
    "CREATE INDEX idx_tasks_workspace_archived_updated ON tasks (workspace_key, archived, updated_at DESC) WHERE deleted = 0",
    "CREATE INDEX idx_tasks_workspace_pinned_updated ON tasks (workspace_key, pinned, updated_at DESC) WHERE deleted = 0",
];

/// `MIGRATED_DDL` 的 sha256（换行连接的规范文本，加载期自校验）。
#[rustfmt::skip]
pub const DDL_HASH: &str = "6f1d4bf85a3d067fda031f5f857ad9fbf5d56250f8271ed7fce1709a222f4051";
