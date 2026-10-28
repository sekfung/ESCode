// Oracle-only：project-storage 的迁移子集（不包含 Repository 写面）。
// 上游 packages/services/src/project-storage/index.ts 的 migrate 分支，去掉 repository/admitTaskIdentity。
import type { DatabaseSync } from "node:sqlite";
import { sqliteMigrations } from "../../../packages/shared/src/sqlite-migrations/index.ts";
import type { ProjectMigrationContext } from "./contract.js";
import {
  DEFAULT_WORKSPACE_CANDIDATES_SQL,
  migrateDefaultWorkspaceTasks,
} from "./default-workspace/migration.js";
import { migrateProjects } from "./migrate-projects.js";
import {
  INITIAL_WORKSPACE_ORDER_SQL,
  migrateInitialWorkspaceOrder,
} from "./migration-order/migration.js";
import {
  PROJECT_MIGRATION_TABLES,
  PROJECT_SCHEMA_SQL,
  TASK_PROJECT_COLUMNS,
} from "./schema.js";

export async function migrateProjectStorage(
  db: DatabaseSync,
  context: ProjectMigrationContext,
) {
  let firstProjectMigration = false;
  return sqliteMigrations.migrate(
    db,
    [
      {
        id: "0005_projects",
        previousIds: ["0001_projects"],
        appVersion: "3.12.0",
        touchedTables: PROJECT_MIGRATION_TABLES,
        sql: `${PROJECT_SCHEMA_SQL}\n-- tasks columns: ${JSON.stringify(TASK_PROJECT_COLUMNS)}\n-- v3 workspace/group first assignment and CLI intent handoff`,
        apply: (database) => {
          migrateProjects(database, context);
          firstProjectMigration = true;
        },
      },
      {
        id: "0006_default_workspace_tasks",
        appVersion: "3.12.0",
        touchedTables: ["tasks", "projects"],
        sql: `${DEFAULT_WORKSPACE_CANDIDATES_SQL}\n-- v1 clear project ownership and soft-delete the unedited system container; preserve task facts`,
        apply: (database) => migrateDefaultWorkspaceTasks(database, context),
      },
      {
        id: "0007_initial_workspace_order",
        appVersion: "3.16.2",
        touchedTables: ["projects", "remote_project_entries"],
        sql: `${INITIAL_WORKSPACE_ORDER_SQL}\n-- seed mixed order only when 0005 runs in this transaction`,
        apply: (database) => {
          database.exec(INITIAL_WORKSPACE_ORDER_SQL);
          if (firstProjectMigration) migrateInitialWorkspaceOrder(database, context);
        },
      },
    ],
    {
      database: "tasks_index",
      backupDirectories: context.backupDirectories,
      now: context.now,
      onBackupSkipped: () => context.warn?.("项目迁移备份因可写卷空间不足跳过"),
    },
  );
}
