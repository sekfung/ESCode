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
    return { tasks, project, ddl };
  } finally {
    db.close();
    await rm(dir, { recursive: true, force: true });
  }
}
