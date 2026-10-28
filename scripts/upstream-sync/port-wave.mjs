#!/usr/bin/env node
// 按上游同步清单执行逐文件三方移植。
//
// 默认干跑，只写报告；传 --apply 才落盘。
//
// 用法：
//   node scripts/upstream-sync/port-wave.mjs --wave W1
//   node scripts/upstream-sync/port-wave.mjs --wave W1 --apply
//   node scripts/upstream-sync/port-wave.mjs --wave W4 --dispositions port-shared,import-test --apply
//   node scripts/upstream-sync/port-wave.mjs --wave W4 --prefix packages/ui/src/ToolCallBlocks
//
// `--prefix`（逗号分隔）按 upstream_path / fork_path 前缀挑选功能域，W4 按域分批时用。
//
// 只处理可自动移植的处置：port-shared / import-test / adopt-docs / adopt-tooling。
// oracle-only、port-rust、port-rust-host、convert-rust-diff、manual-review 一律跳过。

import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

const REPO_ROOT = resolve(import.meta.dirname, "../..");
const MANIFEST = resolve(
  REPO_ROOT,
  process.env.SYNC_MANIFEST ?? "docs/upstream-sync/v3.16.2-manifest.tsv",
);
const REPORT = resolve(REPO_ROOT, process.env.SYNC_REPORT ?? "docs/upstream-sync/port-report.tsv");
const CONFLICT_DIR = resolve(REPO_ROOT, "docs/upstream-sync/conflicts");
const DEFAULT_BASE = "aac47556";
const DEFAULT_UPSTREAM = "refs/remotes/upstream/main";
const AUTO_DISPOSITIONS = new Set(["port-shared", "import-test", "adopt-docs", "adopt-tooling"]);
const SKIP_BASENAMES = new Set(["package.json", "pnpm-lock.yaml", "package-lock.json"]);

const args = process.argv.slice(2);
function argValue(name, fallback) {
  const index = args.indexOf(name);
  return index >= 0 ? args[index + 1] : fallback;
}
const WAVE = argValue("--wave", null);
const APPLY = args.includes("--apply");
const DISPOSITIONS = new Set(
  (argValue("--dispositions", [...AUTO_DISPOSITIONS].join(",")) ?? "").split(",").filter(Boolean),
);
const BASE = argValue("--base", DEFAULT_BASE);
const UPSTREAM = argValue("--upstream", DEFAULT_UPSTREAM);
const PREFIXES = (argValue("--prefix", "") ?? "")
  .split(",")
  .map((prefix) => prefix.trim())
  .filter(Boolean);

// 代理在部分环境里不可达；读取本地对象不需要网络。部分 blob 不在本地 partial clone 里，
// github.com 的 smart-HTTP 不可达但 raw.githubusercontent.com 可用，因此缺失 blob 回退到
// raw 内容（按 commit sha 取，与 git 对象内容一致），避免 `git show` 触发不可用的 promisor fetch。
const RAW_BASE = "https://raw.githubusercontent.com/zai-org/ZCode";
// 镜像按可用性排序：实测 jsDelivr 比 raw.githubusercontent 稳定；两者内容都按 commit 固定。
const CONTENT_MIRRORS = [
  (sha, path) => `https://cdn.jsdelivr.net/gh/zai-org/ZCode@${sha}/${path}`,
  (sha, path) => `${RAW_BASE}/${sha}/${path}`,
];
const RAW_CACHE = resolve(REPO_ROOT, ".git/upstream-sync-raw");
const commitCache = new Map();

/** 代理在部分环境里不可达；git 与 curl 都走直连（raw/API 可达，smart-HTTP 不一定）。 */
function gitEnv() {
  const env = { ...process.env };
  for (const key of [
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "ALL_PROXY",
    "https_proxy",
    "http_proxy",
    "npm_config_proxy",
    "npm_config_https_proxy",
  ]) {
    delete env[key];
  }
  return env;
}

function gitEnvNoLazyFetch() {
  return { ...gitEnv(), GIT_NO_LAZY_FETCH: "1" };
}

function resolveCommit(ref) {
  if (commitCache.has(ref)) return commitCache.get(ref);
  const result = spawnSync("git", ["-C", REPO_ROOT, "rev-parse", `${ref}^{commit}`], {
    env: gitEnv(),
  });
  if (result.status !== 0) return null;
  const sha = result.stdout.toString("utf8").trim();
  commitCache.set(ref, sha);
  return sha;
}

function rawShow(ref, path) {
  const sha = resolveCommit(ref);
  if (!sha) return { ok: false, error: `cannot resolve ${ref}` };
  // 本地缓存：W4 的缺口 blob 较多，预取后重复运行不再访问网络。
  const cached = resolve(RAW_CACHE, sha, path);
  if (existsSync(cached)) {
    return { ok: true, buffer: readFileSync(cached) };
  }
  const result = spawnSync(
    "curl",
    [
      "-4",
      "-sSfL",
      "--retry",
      "2",
      "--retry-delay",
      "1",
      "--connect-timeout",
      "10",
      "--max-time",
      "60",
      CONTENT_MIRRORS[0](sha, path),
    ],
    { maxBuffer: 128 * 1024 * 1024, env: gitEnv() },
  );
  let buffer = result.status === 0 ? result.stdout : null;
  if (!buffer) {
    const fallback = spawnSync(
      "curl",
      [
        "-4",
        "-sSfL",
        "--retry",
        "4",
        "--retry-delay",
        "2",
        "--connect-timeout",
        "15",
        "--max-time",
        "120",
        CONTENT_MIRRORS[1](sha, path),
      ],
      { maxBuffer: 128 * 1024 * 1024, env: gitEnv() },
    );
    if (fallback.status === 0) {
      buffer = fallback.stdout;
    } else {
      return {
        ok: false,
        error:
          fallback.stderr?.toString("utf8").trim() ||
          result.stderr?.toString("utf8").trim() ||
          `raw fetch ${sha}/${path}`,
      };
    }
  }
  mkdirSync(dirname(cached), { recursive: true });
  writeFileSync(cached, buffer);
  return { ok: true, buffer };
}

function gitShow(ref, path) {
  const result = spawnSync("git", ["-C", REPO_ROOT, "show", `${ref}:${path}`], {
    maxBuffer: 128 * 1024 * 1024,
    env: gitEnvNoLazyFetch(),
  });
  if (result.status !== 0) {
    const fallback = rawShow(ref, path);
    if (fallback.ok) return { ...fallback, source: "raw" };
    return {
      ok: false,
      error: `${result.stderr?.toString("utf8").trim() || `git show ${ref}:${path}`} | raw: ${fallback.error}`,
    };
  }
  return { ok: true, buffer: result.stdout };
}

const PROTECTED = [
  "cdn-zcode.z.ai",
  "zcode.z.ai",
  "zcode.ai",
  "zcode-plan",
  "x-zcode-",
  "zcode-websearch-client",
  "zcode-plugins-official",
  ".zcode-plugin",
  "upstream-zcode",
  "zcode/electron/releases",
  "zcode-server.cjs",
  "zcode.cjs",
];

function rebrand(text) {
  let output = text;
  PROTECTED.forEach((token, index) => {
    output = output.split(token).join(`@@SYNC_PROTECT_${index}@@`);
  });
  output = output
    .replaceAll("ZCODE", "ESCODE")
    .replaceAll("ZCode", "ESCode")
    .replaceAll("Zcode", "Escode")
    .replaceAll("zCode", "esCode")
    .replaceAll("zcode", "escode");
  PROTECTED.forEach((token, index) => {
    output = output.split(`@@SYNC_PROTECT_${index}@@`).join(token);
  });
  return output;
}

function isBinary(buffer) {
  return buffer.includes(0);
}

function safeName(path) {
  return path.replace(/[\\/]/g, "__");
}

const rows = readFileSync(MANIFEST, "utf8")
  .split("\n")
  .slice(1)
  .filter(Boolean)
  .map((line) => {
    const [wave, disposition, area, kind, status, path, forkPath, forkState, ...note] =
      line.split("\t");
    return {
      wave,
      disposition,
      area,
      kind,
      status,
      path,
      forkPath,
      forkState,
      note: note.join("\t"),
    };
  });

const selected = rows.filter(
  (row) =>
    (!WAVE || row.wave === WAVE) &&
    DISPOSITIONS.has(row.disposition) &&
    !SKIP_BASENAMES.has(row.path.split("/").pop()) &&
    (PREFIXES.length === 0 ||
      PREFIXES.some((prefix) => row.path.startsWith(prefix) || row.forkPath.startsWith(prefix))),
);

const report = [];
const conflicts = [];

for (const row of selected) {
  const record = (result, detail = "") =>
    report.push({ result, ...row, detail: detail.replace(/\n/g, " ") });

  if (row.status === "D") {
    // 上游删除：自动删除风险高，留给人工确认。
    record("skipped-deleted-upstream", "上游删除，需人工确认");
    continue;
  }

  const target = resolve(REPO_ROOT, row.forkPath);
  const theirs = gitShow(UPSTREAM, row.path);
  if (!theirs.ok) {
    record("error", `读取上游内容失败: ${theirs.error}`);
    continue;
  }
  if (isBinary(theirs.buffer)) {
    record("binary-skip", "二进制文件，需人工处理");
    continue;
  }
  const theirsText = rebrand(theirs.buffer.toString("utf8"));
  const exists = existsSync(target);

  if (!exists) {
    record("would-create", `${theirsText.length} bytes`);
    if (APPLY) {
      mkdirSync(dirname(target), { recursive: true });
      writeFileSync(target, theirsText);
      record("created", `${theirsText.length} bytes`);
    }
    continue;
  }

  const base = gitShow(BASE, row.path);
  if (!base.ok) {
    // 上游 3.16.2 新增、而本地已经按同一上游内容建过（例如分批重跑）：没有基线就保留本地内容，
    // 不把已有文件当成冲突或覆盖。需要人工确认时看该文件的 note/报告。
    record("kept-local-new", `基线无此文件，保留本地内容: ${base.error}`);
    continue;
  }
  const baseText = rebrand(base.buffer.toString("utf8"));
  const oursText = readFileSync(target, "utf8");
  const tmp = resolve(REPO_ROOT, ".git/upstream-sync-tmp");
  mkdirSync(tmp, { recursive: true });
  const oursPath = resolve(tmp, "ours");
  const basePath = resolve(tmp, "base");
  const theirsPath = resolve(tmp, "theirs");
  writeFileSync(oursPath, oursText);
  writeFileSync(basePath, baseText);
  writeFileSync(theirsPath, theirsText);

  const merged = spawnSync("git", ["merge-file", "-p", "--quiet", oursPath, basePath, theirsPath], {
    maxBuffer: 128 * 1024 * 1024,
  });
  const mergedText = merged.stdout.toString("utf8");

  if (merged.status === 0) {
    record("would-merge", "clean");
    if (APPLY) {
      writeFileSync(target, mergedText);
      record("merged", "clean");
    }
  } else if (merged.status >= 1 && merged.status <= 127) {
    // git merge-file 的退出码是冲突数（1–127），不是错误。
    conflicts.push({ row, mergedText, count: merged.status });
    record("conflict", `三方合并冲突 x${merged.status}`);
    if (APPLY) {
      mkdirSync(CONFLICT_DIR, { recursive: true });
      writeFileSync(resolve(CONFLICT_DIR, `${safeName(row.forkPath)}.merge`), mergedText);
    }
  } else {
    record("error", merged.stderr?.toString("utf8").trim() || "merge-file failed");
  }
}

{
  const reportPath = APPLY ? REPORT : REPORT.replace(/\.tsv$/, ".dry.tsv");
  mkdirSync(dirname(reportPath), { recursive: true });
  const header = [
    "result",
    "wave",
    "disposition",
    "area",
    "kind",
    "status",
    "upstream_path",
    "fork_path",
    "fork_state",
    "detail",
  ];
  const tsv = [
    header.join("\t"),
    ...report.map((r) =>
      [
        r.result,
        r.wave,
        r.disposition,
        r.area,
        r.kind,
        r.status,
        r.path,
        r.forkPath,
        r.forkState,
        r.detail,
      ].join("\t"),
    ),
  ].join("\n");
  writeFileSync(reportPath, `${tsv}\n`);
  console.log(`report: ${reportPath}`);
}

function count(key) {
  const out = {};
  for (const row of report) out[row[key]] = (out[row[key]] ?? 0) + 1;
  return out;
}
console.log(
  JSON.stringify(
    {
      apply: APPLY,
      wave: WAVE ?? "all",
      dispositions: [...DISPOSITIONS],
      selected: selected.length,
      results: count("result"),
      conflicts: conflicts.length,
    },
    null,
    2,
  ),
);
if (conflicts.length) {
  console.log("\nconflicts:");
  for (const { row, count } of conflicts.slice(0, 200))
    console.log(`  ${row.forkPath} (${count} conflicts)`);
}
