import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { readFile, readdir, realpath } from "node:fs/promises";
import { join, relative } from "node:path";
import { promisify } from "node:util";
import { resolveSpawnRuntimeOptions } from "./spawn-command.mjs";
import { readLockfileProductionGraph } from "./third-party-lockfile-graph.mjs";

const exec = promisify(execFile);
export const hashBytes = (bytes) => createHash("sha256").update(bytes).digest("hex");
const noticeName =
  /(?:^|[._-])(?:licen[sc]es?|copying|notice|copyright|unlicense|third.party|ofl)(?:[._-]|$)/iu;

export async function readPackageNotices(directory) {
  const files = [];
  async function visit(path) {
    for (const entry of await readdir(path, { withFileTypes: true })) {
      if (["node_modules", ".git", "test", "tests", "fixtures"].includes(entry.name)) continue;
      // Chromium 聚合许可由打包流程经 resources/licenses/electron 单独分发，不进 npm 通知。
      if (entry.isFile() && entry.name === "LICENSES.chromium.html") continue;
      const full = join(path, entry.name);
      if (entry.isDirectory()) await visit(full);
      else if (entry.isFile() && noticeName.test(entry.name)) {
        const bytes = await readFile(full);
        if (bytes.includes(0) || bytes.length === 0) continue;
        files.push({ member: relative(directory, full).replaceAll("\\", "/"), bytes });
      } else if (entry.isFile() && /^readme(?:\.[^/]*)?$/iu.test(entry.name)) {
        const text = await readFile(full, "utf8");
        const match = /^(?:#{1,6}\s+licen[sc]e[^\n]*\n|licen[sc]e\s*\n[=-]+\n)/imu.exec(text);
        if (match) {
          const section = text.slice(match.index).split(/\n(?=#{1,6}\s)/u)[0];
          files.push({
            member: `${relative(directory, full).replaceAll("\\", "/")} (license section)`,
            bytes: Buffer.from(section),
          });
        }
      }
    }
  }
  await visit(directory);
  return files.sort((a, b) => a.member.localeCompare(b.member, "en"));
}

// 解析锁文件（v9）packages 段的平台约束：`  'name@version':` 下的 `    os/cpu/libc: [..]` 行。
export function readLockfilePlatformConstraints(lockfileText) {
  const constraints = new Map();
  let inPackages = false;
  let key = null;
  for (const line of lockfileText.split(/\r?\n/u)) {
    if (/^\S/u.test(line)) {
      inPackages = line === "packages:";
      key = null;
      continue;
    }
    if (!inPackages) continue;
    const header = /^ {2}(?:'([^']+)'|([^\s'][^:]*)):$/u.exec(line);
    if (header) {
      key = header[1] ?? header[2];
      continue;
    }
    const field = key && /^ {4}(os|cpu|libc): \[(.*)\]$/u.exec(line);
    if (field) {
      const values = field[2]
        .split(",")
        .map((value) => value.trim().replace(/^'(.*)'$/u, "$1"))
        .filter(Boolean);
      constraints.set(key, { ...constraints.get(key), [field[1]]: values });
    }
  }
  return constraints;
}

// 照搬 pnpm package-is-installable 的 checkList，保证与实际安装结果一致（含否定项语义）。
function pnpmCheckList(values, list) {
  if (list.length === 1 && list[0] === "any") return true;
  let match = false;
  let blc = 0;
  for (const value of values) {
    for (const item of list) {
      if (item[0] === "!") {
        if (item.slice(1) === value) return false;
        ++blc;
      } else {
        match = match || item === value;
      }
    }
  }
  return match || blc === list.length;
}

// targets.libc 为 null 表示宿主 libc 未知（非 Linux），pnpm 此时不检查 libc。
export function createInstallablePackageFilter(constraints, targets) {
  return (name, version) => {
    const wanted = constraints.get(`${name}@${version.split("(")[0]}`);
    if (!wanted) return true;
    if (wanted.os && !pnpmCheckList(targets.os, wanted.os)) return false;
    if (wanted.cpu && !pnpmCheckList(targets.cpu, wanted.cpu)) return false;
    if (wanted.libc && targets.libc && !pnpmCheckList(targets.libc, wanted.libc)) return false;
    return true;
  };
}

function hostLibc() {
  if (process.platform !== "linux") return null;
  return process.report?.getReport?.()?.header?.glibcVersionRuntime ? "glibc" : "musl";
}

// 与 pnpm checkPlatform 相同：supportedArchitectures 未配置时只取宿主，`current` 替换为宿主值。
export async function readInstallTargets(root) {
  const { parse } = await import("yaml");
  const workspace = parse(await readFile(join(root, "pnpm-workspace.yaml"), "utf8")) ?? {};
  const supported = workspace.supportedArchitectures ?? {};
  const resolveCurrent = (list, current) =>
    (list ?? ["current"]).map((value) => (value === "current" ? current : value));
  const libc = hostLibc();
  return {
    os: resolveCurrent(supported.os, process.platform),
    cpu: resolveCurrent(supported.cpu, process.arch),
    libc: libc ? resolveCurrent(supported.libc, libc) : null,
  };
}

// 修复：pnpm 按 supportedArchitectures 跳过的平台包（如 sharp 的 s390x/wasm32 构建）从不安装、不分发，
// 此前靠按包名的例外名单放行，新依赖一引入就会误报缺失；现在按锁文件平台约束整棵子树跳过。
export function productionPackages(projects, isInstallable = () => true) {
  const own = new Set(projects.map((project) => project.name));
  const required = new Map();
  function dependencies(deps) {
    for (const [alias, info] of Object.entries(deps ?? {})) {
      const name = info.name ?? alias;
      if (!isInstallable(name, info.version)) continue;
      if (!own.has(name) && !name.startsWith("@escode/") && !info.version.startsWith("link:")) {
        required.set(`${name}@${info.version}`, { name, version: info.version });
      }
      dependencies(info.dependencies);
      dependencies(info.optionalDependencies);
    }
  }
  for (const project of projects) {
    dependencies(project.dependencies);
    dependencies(project.optionalDependencies);
  }
  return required;
}

export async function readWorkspaceProductionGraph(root) {
  root = await realpath(root);
  // 两侧都不再依赖 pnpm 的依赖图输出：
  //   - required：直接解析 pnpm-lock.yaml 得生产闭包（scripts/third-party-lockfile-graph.mjs）；
  //   - installed：按真实安装目录递归扫描（scanInstalledPackages）。hoisted 布局下 pnpm ls 会漏掉
  //     嵌套副本（本文件早已因同一原因改用目录匹配），拿它做交叉核对只会误报。
  // 原先那一半 `pnpm ls --lockfile-only` 在 pnpm 9.15.9 与 10.33.2 上都只出现在 --help 里、解析器一律
  // 拒绝（实测），整条 notices 流水线因此恒抛。现在缺包由 missingProductionPackages 判定，
  // 多余包由锁文件全量闭包（含 dev）判定，两个方向都还在，且都基于确定的事实源。
  const [{ projects: lockedProjects, allPackages }, projects] = await Promise.all([
    readLockfileProductionGraph(root),
    readPnpmProjectList(root),
  ]);
  const isInstallable = createInstallablePackageFilter(
    readLockfilePlatformConstraints(await readFile(join(root, "pnpm-lock.yaml"), "utf8")),
    await readInstallTargets(root),
  );
  const required = productionPackages(lockedProjects, isInstallable);
  const installed = await scanInstalledPackages(root, projects);
  // 多余包判定：装上了、但既不在锁文件全量闭包里，也不是 workspace 包本身或 vendored 运行时（见下）。
  const ownNames = new Set(projects.map((project) => project.name));
  const vendored = await readVendoredRuntimePackages(root);
  const unknown = [...installed.keys()].filter((key) => {
    if (allPackages.has(key) || vendored.has(key)) return false;
    const name = key.slice(0, key.startsWith("@") ? key.indexOf("@", 1) : key.indexOf("@"));
    return !ownNames.has(name);
  });
  if (unknown.length > 0) {
    throw new Error(
      `Installed packages are absent from pnpm-lock.yaml. Run pnpm install --frozen-lockfile.\n${unknown.join(", ")}`,
    );
  }
  return { required, projects, installed };
}

/**
 * vendored 运行时的逐包清单（third-party/runtime/sources.json 的 rea 条目等）。
 *
 * 这些包随插件分发、不进 workspace 依赖图，所以锁文件不认识它们 —— 但它们不是"多余包"。
 * 交付声明里它们由 `## Vendored runtimes` 一节单独交代，这里只负责把它们从多余包判定里排除。
 * 清单缺失（例如还没跑过生成器）时返回空集：宁可让门禁报警，也不要静默放行。
 */
async function readVendoredRuntimePackages(root) {
  const allowed = new Set();
  let registries;
  try {
    registries = JSON.parse(await readFile(join(root, "third-party", "runtime", "sources.json"), "utf8"));
  } catch {
    return allowed;
  }
  for (const runtime of Object.values(registries).flat()) {
    if (!runtime?.dependencies) continue;
    try {
      const manifest = JSON.parse(await readFile(join(root, runtime.dependencies), "utf8"));
      for (const item of manifest.packages ?? []) allowed.add(`${item.name}@${item.version}`);
    } catch {
      // 清单缺失不改判定范围：该运行时会被判成多余包并在错误信息里点名，便于发现漏跑生成器。
    }
  }
  return allowed;
}

/**
 * pnpm 的项目清单（name/version/path/private）。`--depth -1` 只列项目、不建依赖图，所以不会像
 * `-r ls --depth Infinity` 那样在这个体量的 monorepo 上开爆 fd（Windows 实测 EMFILE）。
 */
async function readPnpmProjectList(root) {
  const { stdout } = await exec("pnpm", ["-r", "ls", "--json", "--depth", "-1"], {
    cwd: root,
    maxBuffer: 256 * 1024 * 1024,
    ...resolveSpawnRuntimeOptions("pnpm"),
  });
  return JSON.parse(stdout);
}

export async function scanInstalledPackages(root, projects) {
  // pnpm hoisted 布局的 ls.path 仍可能指向不存在的 .pnpm 路径；按真实安装目录和精确版本匹配。
  const installed = new Map();
  const visited = new Set();
  async function scanNodeModules(directory) {
    let actual;
    try {
      actual = await realpath(directory);
    } catch (error) {
      if (error.code === "ENOENT") return;
      throw error;
    }
    if (visited.has(actual)) return;
    visited.add(actual);
    const entries = await readdir(directory, { withFileTypes: true });
    for (const entry of entries) {
      if (entry.name.startsWith(".")) continue;
      const path = join(directory, entry.name);
      if (entry.name.startsWith("@")) {
        for (const scoped of await readdir(path)) await scanPackage(join(path, scoped));
      } else await scanPackage(path);
    }
  }
  async function scanPackage(directory) {
    let pkg;
    try {
      pkg = JSON.parse(await readFile(join(directory, "package.json"), "utf8"));
    } catch (error) {
      if (["ENOENT", "ENOTDIR"].includes(error.code)) return;
      throw error;
    }
    if (pkg.name && pkg.version) installed.set(`${pkg.name}@${pkg.version}`, { pkg, directory });
    await scanNodeModules(join(directory, "node_modules"));
  }
  for (const project of projects) await scanNodeModules(join(project.path, "node_modules"));
  await scanNodeModules(join(root, "node_modules"));
  await scanNodeModules(join(root, "apps/escode-cli/node_modules"));
  return installed;
}

export function missingProductionPackages(required, installed) {
  const missing = [...required].filter(([key]) => !installed.has(key)).map(([, item]) => item);
  for (const item of missing) {
    throw new Error(`Missing installed dependency: ${item.name}@${item.version}`);
  }
  return missing;
}

export async function collectNpmNotices(root, overrides) {
  root = await realpath(root);
  // 标识门禁和声明生成必须扫描同一安装集合，避免嵌套版本只进声明、不进门禁 ——
  // 扫描已由 readWorkspaceProductionGraph 完成，这里复用同一份结果，不再重复走一遍目录树。
  const { required, projects, installed } = await readWorkspaceProductionGraph(root);
  const packages = [];
  const missing = [];
  const notInstalled = missingProductionPackages(required, installed);
  for (const [key, item] of [...required].sort(([a], [b]) => a.localeCompare(b, "en"))) {
    const installedPackage = installed.get(key);
    if (!installedPackage) {
      continue;
    }
    const { pkg, directory } = installedPackage;
    const notices = await readPackageNotices(directory);
    const override = overrides.find((record) => record.package === key);
    if (override?.file) {
      const bytes = await readFile(join(root, override.file));
      if (hashBytes(bytes) !== override.sha256) throw new Error(`Changed upstream notice: ${key}`);
      notices.push({ member: override.source, bytes });
    }
    // README 中仅有 MIT 等标签不能冒充完整许可文件；这种包仍需要版本固定的补充材料。
    if (
      !notices.some(({ member }) => !member.endsWith(" (license section)")) &&
      !override?.acceptedMissingNotice
    )
      missing.push(key);
    packages.push({
      ...item,
      license:
        pkg.license ??
        pkg.licenses?.map((item) => (typeof item === "string" ? item : item.type)).join(" OR ") ??
        override?.license ??
        "(not declared)",
      repository: typeof pkg.repository === "string" ? pkg.repository : pkg.repository?.url,
      ...(override?.acceptedMissingNotice
        ? { acceptedMissingNotice: override.acceptedMissingNotice }
        : {}),
      notices,
    });
  }
  if (missing.length) throw new Error(`Missing complete upstream notices:\n${missing.join("\n")}`);
  return {
    packages,
    notInstalled,
    workspaceManifests: projects.map((project) =>
      relative(root, join(project.path, "package.json")).replaceAll("\\", "/"),
    ),
  };
}
