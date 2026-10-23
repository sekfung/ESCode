// Phase 3 golden freeze：仓内 legacy SQLite 种子。
//
// 种子由真实 TS store 一次性写出（见 ESCODE_RECORD_CLI_GOLDEN 录制），提交为 gzip。
// 正常测试只把种子复制到临时 root，不再引入 TS storage 源码。
import { readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { gunzipSync } from "node:zlib";

const seedDir = resolve("packages/services/tests/fixtures/cli-golden");

/** 录制时写入种子的临时目录占位符。 */
const WORKSPACE = "__WORKSPACE__";
const ROOT = "__ROOT__";

/**
 * 复制仓内种子到 `target`，并把占位符替换成当前临时目录。
 *
 * Rust 导入按 `COALESCE(NULLIF(TRIM(workspace_id),''),directory)` 匹配 workspace，
 * 因此复制后必须改写；消息/部件 JSON 里的 path 与附件 URL 也需要指向当前 root。
 */
export async function copyLegacySeed(
  name: string,
  target: string,
  paths: { workspace: string; root: string },
): Promise<string> {
  await writeFile(target, gunzipSync(await readFile(join(seedDir, `${name}.seed.sqlite.gz`))));
  const db = new DatabaseSync(target);
  try {
    db.exec("PRAGMA journal_mode=DELETE;");
    db.prepare(
      "UPDATE session SET workspace_id=replace(workspace_id, ?, ?), directory=replace(directory, ?, ?), path=replace(path, ?, ?)",
    ).run(WORKSPACE, paths.workspace, WORKSPACE, paths.workspace, WORKSPACE, paths.workspace);
    for (const [table, column] of [
      ["message", "data"],
      ["part", "data"],
      ["session_entry", "data"],
      ["session_input", "payload"],
    ] as const) {
      db.prepare(`UPDATE ${table} SET ${column}=replace(replace(${column}, ?, ?), ?, ?)`).run(
        WORKSPACE,
        paths.workspace,
        ROOT,
        paths.root,
      );
    }
  } finally {
    db.close();
  }
  return target;
}
