// 统计 renderer 首屏静态模块图，并可校验「不应进入首屏」的懒加载 chunk。
//
// 用法：
//   node scripts/audit-renderer-initial-graph.mjs [--dir packages/desktop/out/renderer] [--json]
//   node scripts/audit-renderer-initial-graph.mjs --check
//
// 判定依据：从 index.html 的 entry script 出发，只跟随静态 `from "./x.js"` 与
// `import "./x.js"`；`import("./x.js")` 与 `__vite__mapDeps` 属于动态依赖，不计入。
import { existsSync, readFileSync, statSync } from "node:fs";
import { join, resolve } from "node:path";

const FORBIDDEN_CHUNKS = [
  "SettingsPage",
  "code-viewer",
  "AnimatedSidePanePanel",
  "AutomationsSection",
  "PluginStorePage",
  "KnowledgeSettingsSection",
  "SavedWorkflowLaunchDialog",
  "diffs.worker",
];

const args = process.argv.slice(2);
const dirIndex = args.indexOf("--dir");
const rendererDir =
  dirIndex >= 0 ? resolve(args[dirIndex + 1] ?? "") : resolve("packages/desktop/out/renderer");
const asJson = args.includes("--json");
const check = args.includes("--check");

const htmlPath = join(rendererDir, "index.html");
if (!existsSync(htmlPath)) {
  console.error(`renderer build not found: ${htmlPath}`);
  process.exit(2);
}
const html = readFileSync(htmlPath, "utf8");
const entryMatch = /<script[^>]+src="\.\/assets\/(index-[^"]+\.js)"/.exec(html);
if (!entryMatch) {
  console.error("could not locate renderer entry script in index.html");
  process.exit(2);
}

const assetsDir = join(rendererDir, "assets");
const seen = new Map();
const stack = [join(assetsDir, entryMatch[1])];
const staticImport = /(?:\bfrom\s*|^\s*import\s*)["'](\.\/[^"']+\.js)["']/gm;

while (stack.length > 0) {
  const file = stack.pop();
  if (seen.has(file) || !existsSync(file)) continue;
  const source = readFileSync(file, "utf8");
  seen.set(file, statSync(file).size);
  let match;
  while ((match = staticImport.exec(source))) {
    stack.push(join(assetsDir, match[1].replace(/^\.\//, "")));
  }
}

const entries = [...seen.entries()]
  .map(([file, bytes]) => ({ file: file.slice(assetsDir.length + 1), bytes }))
  .sort((left, right) => right.bytes - left.bytes);
const totalBytes = entries.reduce((sum, entry) => sum + entry.bytes, 0);
const forbidden = entries.filter((entry) =>
  FORBIDDEN_CHUNKS.some((name) => entry.file.includes(name)),
);

if (asJson) {
  console.log(
    JSON.stringify(
      {
        rendererDir,
        entry: entryMatch[1],
        modules: entries.length,
        totalBytes,
        totalMiB: Number((totalBytes / 1024 / 1024).toFixed(2)),
        forbidden,
        largest: entries.slice(0, 15),
      },
      null,
      2,
    ),
  );
} else {
  console.log(
    `static modules=${entries.length} total=${(totalBytes / 1024 / 1024).toFixed(2)} MiB`,
  );
  for (const entry of entries.slice(0, 15)) {
    console.log(`${String(Math.round(entry.bytes / 1024)).padStart(7)} KB  ${entry.file}`);
  }
  if (forbidden.length > 0) {
    console.log("\nforbidden chunks in initial graph:");
    for (const entry of forbidden) console.log(`  ${entry.file}`);
  }
}

if (check && forbidden.length > 0) {
  console.error(`\nrenderer initial graph contains ${forbidden.length} forbidden chunk(s)`);
  process.exit(1);
}
