// Run with node --import tsx. 把 TS tasks-index 的**迁移账本**与**迁移后物理 DDL** 导出为 Rust 内嵌资产。
//
// 提取方式（行为提取，不是文本复制）：先用 project-storage oracle 在临时库上跑完
// 「tasks 基础迁移 + project-storage 迁移」，再读：
//   - `tasks_schema_migration`（id + checksum）→ 期望账本，Rust 打开既有库时逐条对账；
//   - `sqlite_master` 中非内部对象的 SQL → 迁移后的物理 schema，供 Rust 测试夹具与后续切片建库。
// TS 迁移定义（DDL/列/索引/转换）任何变化都会改变这两份产物，`--check` 让 CI 失败，迫使 Rust 同步
// （对应质量报告 §4「DDL 漂移哨兵」欠账）。
import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";

// 经相对路径 import 源码：根 node_modules 不链接工作区包，而包内 `#src/*` 自引用按
// 「导入文件所在包」解析，因此仓储根脚本只能这样拿真实实现（与既有生成器同法）。
// project-storage oracle 只保留迁移子集，避免把 Node Host 的完整 Repository 带进树。
import { buildSchema } from "../scripts/upstream-sync/project-storage-oracle/run.ts";

/** 提取方式（产物形状）变化时递增；Rust 侧加载期断言。 */
const SCHEMA_VERSION = 2;

const built = await buildSchema();
const migrations = built.tasks;
const projectMigrations = built.project;
const ddl = built.ddl;

// 哈希口径用「换行连接的规范文本」而不是 JSON：两端 JSON 序列化对转义的处理不同，
// 换行连接是双方都能逐字节复现的口径。
const expectationsCanonical = migrations.map(([id, checksum]) => `${id}\t${checksum}`).join("\n");
const expectationsHash = createHash("sha256").update(expectationsCanonical).digest("hex");
const projectExpectationsCanonical = projectMigrations
  .map(([id, checksum]) => `${id}\t${checksum}`)
  .join("\n");
const projectExpectationsHash = createHash("sha256")
  .update(projectExpectationsCanonical)
  .digest("hex");
// 资产里写的是压缩掉换行/缩进后的单行文本（Rust 侧字符串字面量更小），
// 哈希必须对**同一份文本**计算，否则 Rust 复现不出来。
const compactDdl = ddl.map((sql) => sql.replace(/\s+/g, " ").trim());
const ddlHash = createHash("sha256").update(compactDdl.join("\n")).digest("hex");

const lines = [
  "//! tasks-index 的迁移账本与物理 DDL，由 `scripts/generate-escode-host-tasks-schema.mjs` 从 TS 生成，勿手改。",
  "//!",
  "//! 期望账本来自 project-storage oracle 在临时库上跑完「tasks 基础迁移 + project-storage 迁移」后的",
  "//! `tasks_schema_migration` 与 `schema_migration`；DDL 来自同一库的 `sqlite_master`。",
  "//! TS 迁移定义变化 → 本文件变化 → `--check` 在 CI 失败。",
  "",
  `/// 生成器版本（提取方式变化时递增）。`,
  `pub const SCHEMA_VERSION: u32 = ${SCHEMA_VERSION};`,
  "",
  "/// 期望的迁移账本 `(id, checksum)`（按 id 升序）。",
  "/// 生成文本长度固定，`#[rustfmt::skip]` 保证生成结果与 rustfmt 无关（否则 `--check` 会被格式化差异干扰）。",
  "#[rustfmt::skip]",
  "pub const EXPECTED_MIGRATIONS: &[(&str, &str)] = &[",
  ...migrations.map(
    ([id, checksum]) => `    (${JSON.stringify(id)}, ${JSON.stringify(checksum)}),`,
  ),
  "];",
  "",
  "/// `EXPECTED_MIGRATIONS` 的 sha256（`\\t`/`\\n` 连接的规范文本，加载期自校验）。",
  "#[rustfmt::skip]",
  `pub const EXPECTATIONS_HASH: &str = ${JSON.stringify(expectationsHash)};`,
  "",
  "/// project-storage 的迁移账本 `(id, checksum)`（`schema_migration`，按 id 升序）。",
  "#[rustfmt::skip]",
  "pub const EXPECTED_PROJECT_MIGRATIONS: &[(&str, &str)] = &[",
  ...projectMigrations.map(
    ([id, checksum]) => `    (${JSON.stringify(id)}, ${JSON.stringify(checksum)}),`,
  ),
  "];",
  "",
  "/// `EXPECTED_PROJECT_MIGRATIONS` 的 sha256。",
  "#[rustfmt::skip]",
  `pub const PROJECT_EXPECTATIONS_HASH: &str = ${JSON.stringify(projectExpectationsHash)};`,
  "",
  "/// 迁移后的物理 DDL（`sqlite_master.sql` 原样，先表后索引；测试夹具与后续切片建库用）。",
  "#[rustfmt::skip]",
  "pub const MIGRATED_DDL: &[&str] = &[",
  ...compactDdl.map((sql) => `    ${JSON.stringify(sql)},`),
  "];",
  "",
  "/// `MIGRATED_DDL` 的 sha256（换行连接的规范文本，加载期自校验）。",
  "#[rustfmt::skip]",
  `pub const DDL_HASH: &str = ${JSON.stringify(ddlHash)};`,
  "",
];
const content = `${lines.join("\n")}`;
const target = new URL(
  "../apps/escode-host/crates/host-services/src/tasks_index/schema.rs",
  import.meta.url,
);

if (process.argv.includes("--check")) {
  if ((await readFile(target, "utf8")) !== content) {
    throw new Error(
      "Rust tasks-index schema asset differs from TS; run node --import tsx scripts/generate-escode-host-tasks-schema.mjs",
    );
  }
} else {
  await writeFile(target, content);
}
