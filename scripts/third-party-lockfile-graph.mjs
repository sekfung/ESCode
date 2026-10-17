// 从 pnpm-lock.yaml 直接构造「生产依赖图」，替代原先 `pnpm ls --lockfile-only` 那一侧。
//
// 为什么不用 pnpm 自己：`--lockfile-only` 在 pnpm 9.15.9 与 10.33.2 上都只出现在 `--help` 里，
// 解析器一律回 `ERROR Unknown option: 'lockfile-only'`（实测），所以那条路在任何 pnpm 版本上都抛错、
// 整个 notices 流水线是死的。直接解析锁文件既恢复原语义（安装树必须与锁文件闭包一致），
// 又跨平台确定、不再受 pnpm 选项变动影响。
//
// 输出形状与 `pnpm ls --prod --json --depth Infinity` 对齐，好让现有的 productionPackages() /
// assertProductionGraphs() 原样消费：
//   [{ name, dependencies: { alias: { name, version, dependencies, optionalDependencies } } }]
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { parse } from "yaml";

/** `1.0.1(react@19.2.7)` → `1.0.1`；`npm:real@1.2.3` → `npm:real@1.2.3`（别名另判）。 */
function versionWithoutPeers(value) {
  const index = value.indexOf("(");
  return index === -1 ? value : value.slice(0, index);
}

/** 快照键 `@scope/name@1.2.3(peer@1)` → `{ name, version }`；无法解析时返回 null 并计数。 */
function splitSnapshotKey(key) {
  const at = key.startsWith("@") ? key.indexOf("@", 1) : key.indexOf("@");
  if (at <= 0) return null;
  return { name: key.slice(0, at), version: versionWithoutPeers(key.slice(at + 1)) };
}

/**
 * 读取锁文件并把每个 workspace 项目的**生产**依赖展开成嵌套结构。
 * devDependencies 不进图 —— 与 `pnpm ls --prod` 一致；传递依赖只走 snapshots 里的 runtime 边。
 */
export async function readLockfileProductionGraph(root) {
  const lockfile = parse(await readFile(join(root, "pnpm-lock.yaml"), "utf8"));
  const snapshots = lockfile.snapshots ?? {};
  const importers = lockfile.importers ?? {};
  // 精确命中优先；peer 后缀与 provider 变体让同一 name@version 可能有多条键，退化为前缀扫描。
  const byName = new Map();
  for (const key of Object.keys(snapshots)) {
    const parsed = splitSnapshotKey(key);
    if (!parsed) continue;
    const bucket = byName.get(parsed.name) ?? [];
    bucket.push({ key, ...parsed });
    byName.set(parsed.name, bucket);
  }

  const stats = { resolved: 0, unresolved: [] };
  /** 收集全量可达包（含 dev），用于判定"装上了但锁文件不认识"的多余包。 */
  const allPackages = new Set();

  /**
   * 依赖边的两种写法：
   *   - importer 的值是 `{ specifier, version }`，snapshot 内部的值是版本字符串；
   *   - 别名依赖写成 `真实包名@版本`（无 `npm:` 前缀），例如
   *     `dependencies["string-width-cjs"] = "string-width@4.2.3"`，快照键用真实包名。
   * 先剥掉 peer 后缀再判形态 —— 否则 `4.0.27(zod@4.6.5)` 里的 `@` 会被误判成别名。
   */
  function parseDependencyValue(alias, rawValue) {
    const value = typeof rawValue === "string" ? rawValue : String(rawValue?.version ?? "");
    const withoutPeers = versionWithoutPeers(value.startsWith("npm:") ? value.slice(4) : value);
    const scoped = withoutPeers.startsWith("@");
    const aliased = scoped ? withoutPeers.indexOf("@", 1) > 0 : withoutPeers.includes("@");
    if (!aliased) return { name: alias, version: withoutPeers, raw: value };
    const at = withoutPeers.lastIndexOf("@");
    return { name: withoutPeers.slice(0, at), version: withoutPeers.slice(at + 1), raw: value };
  }

  function resolveEntry(alias, rawValue) {
    const { name, version, raw } = parseDependencyValue(alias, rawValue);
    allPackages.add(`${name}@${version}`);
    // 三级回退：快照键带 peer 后缀（`name@1.2.3(zod@4)`），先全值精确、再去 peer 精确、最后按名+版本扫。
    const exact = snapshots[`${name}@${raw}`];
    if (exact) {
      stats.resolved += 1;
      return { node: exact, name, version };
    }
    const stripped = snapshots[`${name}@${version}`];
    if (stripped) {
      stats.resolved += 1;
      return { node: stripped, name, version };
    }
    const candidates = byName.get(name)?.filter((item) => item.version === version) ?? [];
    if (candidates.length > 0) {
      stats.resolved += 1;
      return { node: snapshots[candidates[0].key], name, version };
    }
    stats.unresolved.push(`${alias}@${raw}`);
    return { node: undefined, name, version };
  }

  function expand(dependencies, seen) {
    const out = {};
    for (const [alias, rawValue] of Object.entries(dependencies ?? {})) {
      const { node, name, version } = resolveEntry(alias, rawValue);
      const key = `${name}@${version}`;
      if (seen.has(key)) {
        out[alias] = { name, version };
        continue;
      }
      const nested = new Set(seen).add(key);
      const runtime = node?.dependencies;
      const optional = node?.optionalDependencies;
      // name 用解析后的真实包名：别名（`string-width-cjs` → `string-width`）必须按真实包进入闭包，
      // 否则与安装树扫描出的名字对不上。
      out[alias] = {
        name,
        version,
        ...(runtime ? { dependencies: expand(runtime, nested) } : {}),
        ...(optional ? { optionalDependencies: expand(optional, nested) } : {}),
      };
    }
    return out;
  }

  const projects = Object.entries(importers).map(([path, importer]) => ({
    name: path === "." ? (lockfile.importers["."]?.name ?? "escode") : path,
    path,
    // 只取生产依赖：devDependencies 与 `pnpm ls --prod` 的口径一致地被排除。
    dependencies: expand(
      { ...(importer.dependencies ?? {}), ...(importer.optionalDependencies ?? {}) },
      new Set(),
    ),
  }));

  // 全量闭包还要含 dev：门禁要能区分"开发依赖"与"装上了但锁文件不认识的多余包"。
  for (const importer of Object.values(importers)) {
    expand({ ...(importer.devDependencies ?? {}) }, new Set());
  }

  // 静默漏边比报错更糟：它会产出一份"少了几个包"的许可声明，而校验看起来是通过的。
  // 允许未命中的只有 workspace 链接（生产闭包本来就会过滤它们）；其余一律硬失败。
  const tolerated = stats.unresolved.filter((entry) => {
    const value = entry.slice(entry.indexOf("@", entry.startsWith("@") ? 1 : 0) + 1);
    return (
      value.startsWith("link:") || value.startsWith("workspace:") || entry.startsWith("@escode/")
    );
  });
  if (tolerated.length !== stats.unresolved.length) {
    const unexpected = stats.unresolved.filter((entry) => !tolerated.includes(entry));
    throw new Error(
      `pnpm-lock.yaml 里有 ${unexpected.length} 条非 workspace 依赖未能解析成快照（锁文件格式可能变了）：` +
        `${unexpected.slice(0, 5).join(", ")}`,
    );
  }
  return { projects, stats, allPackages };
}
