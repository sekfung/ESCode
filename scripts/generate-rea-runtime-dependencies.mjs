// 生成 REA 运行时的逐包许可清单：third-party/runtime/rea-dependencies.json
//
// 为什么需要它：REA 运行时（rea-agents 及其依赖树，约 132 个包）按目标平台安装、不入库，
// 无法像其它 vendored 源码那样逐字节哈希登记（平台差异会让哈希在 CI 上漂移）。替代办法是把
// 「我们分发了哪些包、各自声明什么许可、包里有没有许可原文」记录下来 —— 由 npm 元数据生成，
// 与 pin 的版本一起入库，升级时随 upgrade-rea.mjs 重新生成。
//
// 用法：node scripts/generate-rea-runtime-dependencies.mjs
import { createHash } from "node:crypto";
import { existsSync, readdirSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const pluginRoot = join(repoRoot, "apps", "escode-cli", "packages", "rea-plugin");
const nodeModules = join(pluginRoot, "node_modules");
const outputPath = join(repoRoot, "third-party", "runtime", "rea-dependencies.json");

const pinnedVersion = JSON.parse(readFileSync(join(pluginRoot, "package.json"), "utf8"))
  ?.escodePlugin?.reaAgentsVersion?.trim();
if (!pinnedVersion) throw new Error("插件 package.json 缺少 escodePlugin.reaAgentsVersion");
if (!existsSync(nodeModules)) {
  throw new Error(`运行时未安装：${nodeModules}（先跑 pnpm --filter @escode/rea-plugin build）`);
}

/**
 * 收集运行时树里的全部包，含**嵌套副本**（npm 的嵌套布局下同一包可能有多份不同版本，例如
 * incur/node_modules/@modelcontextprotocol/server@2.0.0-alpha.4）。清单要覆盖我们真正分发的每个包，
 * 所以不能只扫顶层 —— 漏掉的嵌套副本会被 notices 的多余包门禁判成"装上了但锁文件不认识"。
 */
function collectPackageDirs(root) {
  const dirs = [];
  const visited = new Set();
  function walkNodeModules(directory) {
    let entries;
    try {
      entries = readdirSync(directory, { withFileTypes: true });
    } catch {
      return;
    }
    for (const entry of entries) {
      if (!entry.isDirectory() || entry.name.startsWith(".")) continue;
      const path = join(directory, entry.name);
      if (entry.name.startsWith("@")) {
        for (const scoped of readdirSync(path, { withFileTypes: true })) {
          if (scoped.isDirectory()) record(join(path, scoped.name));
        }
        continue;
      }
      record(path);
    }
  }
  function record(packageDir) {
    if (!existsSync(join(packageDir, "package.json"))) return;
    let real;
    try {
      real = realpathSync(packageDir);
    } catch {
      return;
    }
    if (visited.has(real)) return;
    visited.add(real);
    dirs.push(packageDir);
    walkNodeModules(join(packageDir, "node_modules"));
  }
  walkNodeModules(root);
  return dirs;
}

function licenseText(manifest) {
  if (typeof manifest.license === "string") return manifest.license;
  if (manifest.license && typeof manifest.license === "object" && manifest.license.type) {
    return String(manifest.license.type);
  }
  if (Array.isArray(manifest.licenses)) {
    return manifest.licenses
      .map((item) => (typeof item === "string" ? item : item?.type))
      .filter(Boolean)
      .join(" OR ");
  }
  return "(not declared in package metadata)";
}

const packages = [];
for (const directory of collectPackageDirs(nodeModules)) {
  const manifestPath = join(directory, "package.json");
  if (!existsSync(manifestPath)) continue;
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  if (typeof manifest.name !== "string" || typeof manifest.version !== "string") continue;
  // 包里是否自带许可原文：只记事实，不代替原文（清单里不复制正文，避免膨胀与误标版权人）。
  const licenseFile = readdirSync(directory).find((name) => /^(LICEN[CS]E|COPYING|NOTICE)/i.test(name));
  packages.push({
    name: manifest.name,
    version: manifest.version,
    license: licenseText(manifest),
    ...(licenseFile ? { licenseFile } : {}),
    // 带 os/cpu 约束的包按目标平台解析，各平台装出来的集合不同 —— 标记出来，让稳定哈希只覆盖其余部分。
    ...(manifest.os || manifest.cpu ? { platformRestricted: true } : {}),
  });
}
packages.sort((left, right) => left.name.localeCompare(right.name, "en"));

const reaAgents = packages.find((item) => item.name === "rea-agents");
if (reaAgents?.version !== pinnedVersion) {
  throw new Error(`运行时版本不符：已安装 ${reaAgents?.version}，pin 是 ${pinnedVersion}`);
}

// 稳定哈希：只覆盖平台无关的包，这样在 Windows 开发机与 ubuntu CI 上重生成结果一致，
// 跨平台的漂移检查才可用（平台特定包另列，见 platformRestricted）。
const stable = packages.filter((item) => !item.platformRestricted);
const stableHash = createHash("sha256")
  .update(stable.map((item) => `${item.name}@${item.version} ${item.license}`).join("\n"))
  .digest("hex");

writeFileSync(
  outputPath,
  `${JSON.stringify(
    {
      runtime: `rea-agents@${pinnedVersion}`,
      generatedBy: "scripts/generate-rea-runtime-dependencies.mjs",
      note: "许可标识取自各包 package.json 的 license/licenses 字段；licenseFile 表示包里自带许可原文。完整正文随运行时一起分发（见 third-party/runtime/sources.json 的 rea 条目）。platformRestricted 的包按目标平台解析，各平台不同，因此不参与 stableHash。",
      stablePackageCount: stable.length,
      stableHash,
      packages,
    },
    null,
    2,
  )}\n`,
);
process.stdout.write(
  `rea runtime dependencies: ${packages.length} 个包（平台无关 ${stable.length}，平台特定 ${packages.length - stable.length}）-> ${outputPath}\n` +
    `stableHash: ${stableHash.slice(0, 16)}…；未声明许可的: ${packages.filter((item) => item.license.startsWith("(not declared")).length}\n`,
);
