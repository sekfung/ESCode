#!/usr/bin/env node
// 收口 port-wave 产生的冲突文件：
// - 冲突块的 theirs 命中已删除功能域 → 取 ours（丢弃上游片段）
// - ours 为空、theirs 保留 → 取 theirs
// - 两侧都有内容且不是删除域 → 保留冲突标记，交给人工
//
// 用法：node scripts/upstream-sync/resolve-conflicts.mjs [--apply]

import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const REPO_ROOT = resolve(import.meta.dirname, "../..");
const REPORT = resolve(REPO_ROOT, "docs/upstream-sync/port-report.tsv");
const CONFLICT_DIR = resolve(REPO_ROOT, "docs/upstream-sync/conflicts");
const APPLY = process.argv.includes("--apply");

const DELETED_FRAGMENT =
  /WebRemoteControl|web-?remote|webRemote|telemetry|Telemetry|ResourceSample|McpResource|mobile|Mobile|conversation-?share|off-?peak|bots?[A-Z_.-]|bots\b|feedback|coding-?plan|bigmodel|cloud-?content|marketing-?touch|oauth|OAuth|arms/i;

function safeName(path) {
  return path.replace(/[\\/]/g, "__");
}

const rows = readFileSync(REPORT, "utf8")
  .split("\n")
  .slice(1)
  .filter(Boolean)
  .map((line) => line.split("\t"));
const conflictRows = rows.filter((row) => row[0] === "conflict");

let resolvedFiles = 0;
let stillConflicted = 0;

for (const row of conflictRows) {
  const forkPath = row[7];
  const mergePath = resolve(CONFLICT_DIR, `${safeName(forkPath)}.merge`);
  if (!existsSync(mergePath)) {
    console.log(`missing merge file: ${mergePath}`);
    stillConflicted += 1;
    continue;
  }
  const text = readFileSync(mergePath, "utf8");
  const lines = text.split("\n");
  const output = [];
  let unresolved = 0;

  for (let i = 0; i < lines.length; i += 1) {
    if (!lines[i].startsWith("<<<<<<< ")) {
      output.push(lines[i]);
      continue;
    }
    const ours = [];
    const theirs = [];
    i += 1;
    while (i < lines.length && !lines[i].startsWith("=======")) {
      ours.push(lines[i]);
      i += 1;
    }
    i += 1;
    while (i < lines.length && !lines[i].startsWith(">>>>>>> ")) {
      theirs.push(lines[i]);
      i += 1;
    }

    const oursText = ours.join("\n");
    const theirsText = theirs.join("\n");
    const deleted = DELETED_FRAGMENT.test(theirsText);

    if (deleted) {
      output.push(...ours);
    } else if (oursText.trim() === "") {
      output.push(...theirs);
    } else {
      // 真语义冲突：保留标记，人工处理。
      unresolved += 1;
      output.push("<<<<<<< ours");
      output.push(...ours);
      output.push("=======");
      output.push(...theirs);
      output.push(">>>>>>> upstream");
    }
  }

  if (unresolved === 0) {
    resolvedFiles += 1;
    if (APPLY) writeFileSync(resolve(REPO_ROOT, forkPath), output.join("\n"));
    console.log(`resolved: ${forkPath}`);
  } else {
    stillConflicted += 1;
    console.log(`unresolved: ${forkPath} (${unresolved} hunks)`);
  }
}

console.log(JSON.stringify({ apply: APPLY, resolvedFiles, stillConflicted }, null, 2));
