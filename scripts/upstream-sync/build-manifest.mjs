#!/usr/bin/env node
// 生成上游 ZCode 同步的逐文件清单。清单只做分类，不修改工作区。
//
// 用法：
//   node scripts/upstream-sync/build-manifest.mjs
//
// 可用环境变量覆盖：
//   SYNC_BASE      合并基线上游提交（默认 aac47556，即上游 v3.15.1）
//   SYNC_UPSTREAM  上游目标引用（默认 refs/remotes/upstream/main）
//   SYNC_OUT       输出 TSV 路径（默认 docs/upstream-sync/v3.16.2-manifest.tsv）

import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

const REPO_ROOT = resolve(import.meta.dirname, "../..");
const BASE = process.env.SYNC_BASE ?? "aac47556";
const UPSTREAM = process.env.SYNC_UPSTREAM ?? "refs/remotes/upstream/main";
const OUT = resolve(REPO_ROOT, process.env.SYNC_OUT ?? "docs/upstream-sync/v3.16.2-manifest.tsv");

// 上游更名前的路径前缀。只映射项目内已知的重命名，不做全文替换：
// third-party/upstream-zcode、CDN 对象前缀等按更名 spec 保留原名。
function mapForkCandidates(path) {
  const prefixed = path
    .replace(/^apps\/zcode-cli\//, "apps/escode-cli/")
    .replace(/^packages\/zcode-/, "packages/escode-");
  const rebranded = path.replace(/zcode/g, "escode");
  return [...new Set([prefixed, rebranded])];
}

// 用户已确认删除、且不随上游恢复的功能域。命中即整文件排除；
// 共享文件里夹带的单个片段由后续波次按“剔片段、留其余”处理。
const DELETED_DOMAIN = [
  /^packages\/web\//,
  /^packages\/android/,
  /^packages\/web-share\//,
  /^packages\/share-ui\//,
  /conversation-?share/i,
  /(^|\/)bots(\.|\/|-)/i,
  /^packages\/services\/src\/bots\//,
  /bot-project-runtime/,
  /channel-reply|reply-to-channel|replyToChannel/i,
  /off-?peak/i,
  /(^|\/)feedback(\/|$)/i,
  /(^|\/)feedback[-_.]/i,
  /feedback\.md$/i,
  /web-?remote/i,
  /mobile-ui/i,
  /conversation-telemetry/i,
  /telemetry/i,
  /marketing-?touch/i,
  /coding-?plan/i,
  /cloud-?content/i,
  /\/bigmodel\//,
  /^packages\/services\/src\/oauth\//,
];

// 命中这些路径的 Node CLI 改动会改变 runtime 可观察行为，需要 Rust 跟进。
const RUST_FEATURE =
  /tool\/search|tool-search|deferred|tool-result-settlement|storage\/session-store|session-store|migrations|model\/runner|model-execution|provider-endpoint-routing|compact|turn-model|subagent|workflow-blocked|runtime-reminder|zcode-protocol-v4|session-execution|residency|mcp\/pool/i;

const SHARED_PROTOCOL =
  /zcode-protocol|remote-workspace-identity|server-remote|tool-identity|sqlite-migrations/;

function isDeleted(path) {
  return DELETED_DOMAIN.some((re) => re.test(path));
}

function isTest(path) {
  return (
    path.includes("/test/") ||
    path.includes("/tests/") ||
    path.includes("/e2e/") ||
    path.includes("/__tests__/") ||
    path.includes("/fixtures/") ||
    /\.(test|spec)\.(ts|tsx|mjs|js)$/.test(path) ||
    path.startsWith("scripts/test/")
  );
}

function isDoc(path) {
  return (
    path.startsWith("docs/") ||
    path.includes("/docs/") ||
    /\.(md|mdx)$/.test(path) ||
    /\.(png|svg|ico|icns|jpg|webp)$/.test(path)
  );
}

function isTooling(path) {
  return (
    path.startsWith("scripts/") ||
    path.startsWith("config/") ||
    path.startsWith(".github/") ||
    path.startsWith(".agents/") ||
    path.startsWith(".vscode/") ||
    path.startsWith("harness/") ||
    /^(package\.json|package-lock\.json|pnpm-lock\.yaml|pnpm-workspace\.yaml|tsconfig\.base\.json|vitest\.config\.ts|vitest\.setup\.ts|knip\.json|lint-staged\.config\.mjs|architecture-policy\.yaml|\.oxlintrc\.json|\.oxfmtrc\.json|\.gitignore|\.gitattributes|mise\.toml)$/.test(
      path,
    )
  );
}

function areaOf(path) {
  if (path.startsWith("apps/zcode-cli/")) return "cli";
  if (path.startsWith("packages/desktop/src/host/")) return "node-host";
  if (path.startsWith("packages/services/src/zcode-agent/")) return "node-runtime";
  if (/source-server|sourceServer|connectSourceServer|serverRemote|sourceSettings/.test(path))
    return "source-server";
  if (
    /project-storage|project-catalog|project-workspace|sqlite-migrations|startup-storage|\/project\//.test(
      path,
    )
  )
    return "project-data";
  if (
    path.startsWith("packages/ui/") ||
    path.startsWith("packages/desktop/src/renderer/") ||
    path.startsWith("packages/desktop/src/preload/")
  )
    return "ui";
  if (
    path.startsWith("packages/shared/") ||
    path.startsWith("packages/rpc/") ||
    path.startsWith("packages/client/") ||
    path.startsWith("packages/provider/") ||
    path.startsWith("packages/provider-node/") ||
    path.startsWith("packages/zcode-cua/")
  )
    return "shared";
  if (path.startsWith("packages/desktop/")) return "desktop";
  if (path.startsWith("packages/services/")) return "services";
  if (path.startsWith("packages/server/") || path.startsWith("packages/zcode-server-cli/"))
    return "server";
  if (path.startsWith("apps/dev-docs/")) return "docs-site";
  return "other";
}

function waveOf(area) {
  if (area === "deleted") return "-";
  if (area === "project-data" || area === "shared") return "W1";
  if (area === "cli" || area === "node-runtime") return "W2";
  if (area === "node-host" || area === "source-server" || area === "services" || area === "server")
    return "W3";
  if (area === "ui" || area === "desktop") return "W4";
  return "W5";
}

function classify(path, forkExists) {
  const kind = isDoc(path) ? "doc" : isTest(path) ? "test" : isTooling(path) ? "tooling" : "src";
  const area = areaOf(path);

  // 只有“命中删除域且 fork 已没有同一路径”才整文件排除；
  // 删除域的文档一律排除；fork 仍保留源码/测试时走人工核对，避免误删仍在用的能力。
  if (isDeleted(path)) {
    if (kind === "doc" || !forkExists) {
      return {
        kind,
        area: "deleted",
        disposition: "exclude-deleted",
        wave: "-",
        note: kind === "doc" ? "已删除功能域文档" : "已删除功能域，用户确认不恢复",
      };
    }
    return {
      kind,
      area,
      disposition: "manual-review",
      wave: "W5",
      note: "命中删除域但 fork 仍保留同一路径，需剔片段或确认",
    };
  }

  return { kind, area, ...dispositionFor(kind, area, path) };
}

function dispositionFor(kind, area, path) {
  if (kind === "doc") {
    return { disposition: "adopt-docs", wave: "W5", note: "文档按品牌映射摄入" };
  }
  if (kind === "tooling") {
    return { disposition: "adopt-tooling", wave: "W5", note: "脚本/配置/治理工具" };
  }
  if (kind === "test") {
    if (
      ["cli", "node-runtime", "node-host", "services", "server", "source-server"].includes(area) ||
      (area === "project-data" && path.startsWith("packages/services/"))
    ) {
      return {
        disposition: "convert-rust-diff",
        wave: area === "cli" || area === "node-runtime" ? "W2" : "W3",
        note: "Node/Rust 行为差分语料，不直接导入 Node 单测",
      };
    }
    return {
      disposition: "import-test",
      wave: waveOf(area),
      note: "随所属波次导入并按品牌映射",
    };
  }

  switch (area) {
    case "cli":
      return RUST_FEATURE.test(path)
        ? { disposition: "port-rust", wave: "W2", note: "Node runtime 行为，移植到 Rust" }
        : { disposition: "oracle-only", wave: "W2", note: "Node 内部结构改动，仅作 oracle" };
    case "node-runtime":
      return { disposition: "port-rust", wave: "W2", note: "Node runtime 行为，移植到 Rust" };
    case "node-host":
      return {
        disposition: "port-rust-host",
        wave: "W3",
        note: "Node Host 行为，移植到 Rust Host",
      };
    case "source-server":
      if (path.startsWith("packages/shared/")) {
        return { disposition: "port-shared", wave: "W3", note: "Source Server 共享协议与契约" };
      }
      return {
        disposition: "port-rust-host",
        wave: "W3",
        note: "Source Server 本轮只跟协议/只读展示，远程仍走 Node Host",
      };
    case "project-data":
      if (path.startsWith("packages/shared/")) {
        return { disposition: "port-shared", wave: "W1", note: "数据模型/迁移契约" };
      }
      return {
        disposition: "port-rust-host",
        wave: "W3",
        note: "项目/存储模型，随 Rust Host 状态生成对齐",
      };
    case "ui":
    case "desktop":
      return { disposition: "port-shared", wave: "W4", note: "TS/UI 代码按品牌映射三方合并" };
    case "shared":
      return {
        disposition: "port-shared",
        wave: SHARED_PROTOCOL.test(path) ? "W1" : "W1",
        note: SHARED_PROTOCOL.test(path) ? "V4 协议/身份/迁移契约" : "共享类型与客户端",
      };
    case "services":
      if (
        /\/session\/|project|storage|source-server|tasks?Database|task-facts|task-groups|zcode-session/.test(
          path,
        )
      ) {
        return {
          disposition: "port-rust-host",
          wave: "W3",
          note: "Host 状态/数据模型，Rust Host 需镜像",
        };
      }
      return {
        disposition: "manual-review",
        wave: "W3",
        note: "services 改动需逐项判断 Host/共享归属",
      };
    case "server":
      return { disposition: "port-shared", wave: "W3", note: "远端 Server/打包链路" };
    case "docs-site":
      return { disposition: "adopt-docs", wave: "W5", note: "文档站点" };
    default:
      return { disposition: "manual-review", wave: "W5", note: "根级/其他文件" };
  }
}

function git(args) {
  return execFileSync("git", ["-C", REPO_ROOT, ...args], {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
}

const delta = git(["diff", "--no-renames", "--name-status", BASE, UPSTREAM])
  .split("\n")
  .filter(Boolean)
  .map((line) => {
    const [status, ...rest] = line.split("\t");
    return { status: status[0] === "T" ? "M" : status[0], path: rest.join("\t") };
  });

const forkFiles = new Set(
  git(["ls-tree", "-r", "--name-only", "HEAD"]).split("\n").filter(Boolean),
);

const rows = delta.map(({ status, path }) => {
  const candidates = mapForkCandidates(path);
  const existingForkPath = candidates.find((candidate) => forkFiles.has(candidate));
  const forkPath = existingForkPath ?? candidates[0];
  const exists = Boolean(existingForkPath);
  const classified = classify(path, exists);
  const meta =
    (status === "M" && !exists) || (status === "D" && exists)
      ? {
          ...classified,
          disposition:
            classified.disposition === "exclude-deleted" ? "exclude-deleted" : "manual-review",
          note:
            status === "M"
              ? "上游修改但 fork 无同名文件，需确认改名/删除"
              : "上游删除但 fork 仍存在，需确认是否同步删除",
        }
      : classified;
  const forkState =
    status === "A" ? (exists ? "collision" : "new") : exists ? "present" : "missing";
  return {
    wave: meta.wave,
    disposition: meta.disposition,
    area: meta.area,
    kind: meta.kind,
    status,
    path,
    forkPath,
    forkState,
    note: meta.note,
  };
});

const order = { W1: 1, W2: 2, W3: 3, W4: 4, W5: 5, "-": 9 };
rows.sort(
  (a, b) =>
    order[a.wave] - order[b.wave] || a.area.localeCompare(b.area) || a.path.localeCompare(b.path),
);

const header = [
  "wave",
  "disposition",
  "area",
  "kind",
  "status",
  "upstream_path",
  "fork_path",
  "fork_state",
  "note",
];
const tsv = [
  header.join("\t"),
  ...rows.map((r) =>
    [r.wave, r.disposition, r.area, r.kind, r.status, r.path, r.forkPath, r.forkState, r.note].join(
      "\t",
    ),
  ),
].join("\n");

mkdirSync(dirname(OUT), { recursive: true });
writeFileSync(OUT, `${tsv}\n`);

function countBy(key) {
  const counts = {};
  for (const row of rows) counts[row[key]] = (counts[row[key]] ?? 0) + 1;
  return Object.fromEntries(Object.entries(counts).sort((a, b) => b[1] - a[1]));
}

const summary = {
  base: BASE,
  upstream: UPSTREAM,
  upstreamHead: git(["rev-parse", UPSTREAM]).trim(),
  total: rows.length,
  byDisposition: countBy("disposition"),
  byWave: countBy("wave"),
  byArea: countBy("area"),
  byKind: countBy("kind"),
};
const summaryPath = OUT.replace(/\.tsv$/, ".summary.json");
writeFileSync(summaryPath, `${JSON.stringify(summary, null, 2)}\n`);

console.log(`manifest: ${OUT}`);
console.log(`summary:  ${summaryPath}`);
console.log(JSON.stringify(summary, null, 2));
