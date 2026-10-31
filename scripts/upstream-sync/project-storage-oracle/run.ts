// Oracle-only：在临时库上跑「tasks 基础迁移 + project-storage 迁移」，导出两份迁移账本与物理 DDL。
// 供 scripts/generate-escode-host-tasks-schema.mjs 使用。
import { mkdtemp, rm } from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { runTasksDatabaseMigrations } from "../../../packages/services/src/session/tasksDatabase/migrations.ts";
import { migrateProjectStorage } from "./project-migrate.js";

const { DatabaseSync } = createRequire(import.meta.url)("node:sqlite");

export async function buildSchema() {
  const dir = await mkdtemp(join(tmpdir(), "escode-host-tasks-schema-"));
  const db = new DatabaseSync(join(dir, "tasks-index.sqlite"));
  const tasksOnly = new DatabaseSync(join(dir, "tasks-only.sqlite"));
  try {
    db.exec("PRAGMA foreign_keys = ON");
    runTasksDatabaseMigrations(db, {});
    await migrateProjectStorage(db, { backupDirectories: [join(dir, "backup")] });
    const tasks = db
      .prepare("SELECT id, checksum FROM tasks_schema_migration ORDER BY id")
      .all()
      .map((row: { id: unknown; checksum: unknown }) => [String(row.id), String(row.checksum)]);
    const project = db
      .prepare("SELECT id, checksum FROM schema_migration ORDER BY id")
      .all()
      .map((row: { id: unknown; checksum: unknown }) => [String(row.id), String(row.checksum)]);
    const ddl = db
      .prepare(
        `SELECT sql FROM sqlite_master
         WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%'
         ORDER BY CASE type WHEN 'table' THEN 0 WHEN 'index' THEN 1 ELSE 2 END, name`,
      )
      .all()
      .map((row: { sql: unknown }) => String(row.sql));
    // project-storage 迁移新增的对象 = 全量 DDL 中 tasks-only 库里不存在的名字。
    // 列补齐（tasks/automations 的 project 列）已由 tasks 迁移负责，因此这里只导出
    // 新建表/索引；`schema_migration` 由启动 bootstrap 自己创建，不放进 DDL。
    tasksOnly.exec("PRAGMA foreign_keys = ON");
    runTasksDatabaseMigrations(tasksOnly, {});
    const tasksDdl = tasksOnly
      .prepare(
        `SELECT sql FROM sqlite_master
         WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%'
         ORDER BY CASE type WHEN 'table' THEN 0 WHEN 'index' THEN 1 ELSE 2 END, name`,
      )
      .all()
      .map((row: { sql: unknown }) => String(row.sql));
    const tasksOnlyNames = new Set(
      tasksOnly
        .prepare("SELECT name FROM sqlite_master WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%'")
        .all()
        .map((row: { name: unknown }) => String(row.name)),
    );
    const projectObjects = db
      .prepare(
        `SELECT type, name, sql FROM sqlite_master
         WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%'
         ORDER BY CASE type WHEN 'table' THEN 0 WHEN 'index' THEN 1 ELSE 2 END, name`,
      )
      .all()
      .filter(
        (row: { name: unknown }) =>
          !tasksOnlyNames.has(String(row.name)) && String(row.name) !== "schema_migration",
      ) as { type: string; name: string; sql: string }[];
    // project-storage 的 0005 会给既有 tasks 表补列（host_id/root_path/project_id 等），
    // 这些 ALTER 不会出现在 sqlite_master 的差集里；必须按列差异单独导出，
    // 否则空库 bootstrap 创建 tasks 索引时会报 `no such column: host_id`。
    const columnAlters: string[] = [];
    const sharedTables = tasksOnly
      .prepare(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
      )
      .all()
      .map((row: { name: unknown }) => String(row.name));
    for (const table of sharedTables) {
      const existing = new Set(
        (tasksOnly.prepare(`PRAGMA table_info("${table}")`).all() as { name: string }[]).map(
          (row) => row.name,
        ),
      );
      const fullColumns = db.prepare(`PRAGMA table_info("${table}")`).all() as {
        name: string;
        type: string;
        notnull: number;
        dflt_value: string | null;
      }[];
      for (const column of fullColumns) {
        if (existing.has(column.name)) continue;
        const definition = [
          column.type,
          column.notnull ? "NOT NULL" : "",
          column.dflt_value === null ? "" : `DEFAULT ${column.dflt_value}`,
        ]
          .filter(Boolean)
          .join(" ");
        columnAlters.push(`ALTER TABLE "${table}" ADD COLUMN "${column.name}" ${definition}`);
      }
    }
    const projectDdl = [
      ...projectObjects.filter((row) => row.type === "table").map((row) => row.sql),
      ...columnAlters,
      ...projectObjects.filter((row) => row.type !== "table").map((row) => row.sql),
    ];
    return { tasks, project, ddl, projectDdl, tasksDdl };
  } finally {
    db.close();
    tasksOnly.close();
    await rm(dir, { recursive: true, force: true });
  }
}
