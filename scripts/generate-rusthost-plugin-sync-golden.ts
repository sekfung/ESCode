// Phase 3 golden freeze：rustHostPluginSyncParity.test.ts 的 TS `createPluginSyncService` 观察录制。
// 一次性脚本；删除 TS 实现前用于重录。`--check` 校验仓内 golden 与当前 TS 观察一致。
//
// 候选 `id` 是 `${pluginId}:${canonicalPath}` 的 sha256，路径不同会得到不同 id；因此夹具必须使用
// 固定根目录（与 rustHostFileIndex 一致），测试侧再把 `$FIXTURE_ROOT` 替换为同一根目录。
// 归档 tar 头包含源文件 mtime，录制与回放前统一冻结为固定时间，只归一 metadata 条目的导出时刻。
import { deepEqual } from "node:assert/strict";
import { createHash } from "node:crypto";
import { gunzipSync } from "node:zlib";
import { mkdir, readFile, readdir, realpath, rm, utimes, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { createPluginSyncService } from "../packages/services/src/node.ts";

const FIXTURE_ROOT = "/tmp/escode-rust-plugin-sync-fixture";
const FIXED_MTIME_SECONDS = 1_600_000_000;

async function plugin(root: string, name: string): Promise<void> {
  await mkdir(join(root, ".escode-plugin"), { recursive: true });
  await writeFile(
    join(root, ".escode-plugin", "plugin.json"),
    JSON.stringify({ name, version: "1.0.0" }),
  );
  await writeFile(join(root, "README.md"), "# readme\n");
}

/** 递归固定 mtime（先子后父）：tar 头按秒记录源 mtime，必须跨录制/回放保持一致。 */
async function freezeMtimes(path: string): Promise<void> {
  const time = new Date(FIXED_MTIME_SECONDS * 1000);
  const entries = await readdir(path, { withFileTypes: true });
  for (const entry of entries) {
    const child = join(path, entry.name);
    if (entry.isDirectory()) await freezeMtimes(child);
    else await utimes(child, time, time);
  }
  await utimes(path, time, time);
}

/** metadata 条目的 mtime 是导出时刻（TS `Date.now()`）；归一并重算 checksum 后其余 tar 字节必须全等。 */
function normalizeMetadataMtime(tar: Buffer): Buffer {
  const copy = Buffer.from(tar);
  const marker = Buffer.from(".escode-plugin-sync.json");
  const start = copy.indexOf(marker);
  if (start >= 0) {
    copy.fill(0, start + 136, start + 148);
    copy.fill(0x20, start + 148, start + 156);
    let checksum = 0;
    for (let index = start; index < start + 512; index += 1) checksum += copy[index]!;
    copy.write(checksum.toString(8).padStart(6, "0"), start + 148, 6, "ascii");
    copy[start + 154] = 0;
    copy[start + 155] = 0x20;
  }
  return copy;
}

/** 绝对临时路径归一化：测试运行时把 `$FIXTURE_ROOT` 换成同一固定夹具根。 */
function normalize<T>(value: T, root: string): T {
  return JSON.parse(JSON.stringify(value).split(root).join("$FIXTURE_ROOT")) as T;
}

/**
 * 超过 1 MiB 的字符串只保留 sha256，避免把大 payload 写进仓库；
 * 测试侧对 Rust 输出做同一 seal 后比较（与 rustHostAttachments 一致）。
 */
const LARGE_STRING = 1024 * 1024;
function sealLargeStrings<T>(value: T): T {
  if (typeof value === "string") {
    return (
      value.length > LARGE_STRING
        ? `sha256:${createHash("sha256").update(value).digest("hex")}`
        : value
    ) as T;
  }
  if (Array.isArray(value)) return value.map((item) => sealLargeStrings(item)) as T;
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, sealLargeStrings(item)]),
    ) as T;
  }
  return value;
}

await rm(FIXTURE_ROOT, { recursive: true, force: true });
await mkdir(FIXTURE_ROOT, { recursive: true });
// /tmp 在部分平台是软链；candidate.path / id 使用 canonical root，录制与回放都用 realpath。
const root = await realpath(FIXTURE_ROOT);
const home = join(root, "home");
const dataBaseDir = join(root, "data");
for (const path of [home, dataBaseDir]) await mkdir(path, { recursive: true });
const alpha = join(home, "src", "alpha");
const beta = join(home, "src", "beta");
await plugin(alpha, "alpha");
await plugin(beta, "beta");
await mkdir(join(beta, "skills"), { recursive: true });
await writeFile(join(beta, "skills", "SKILL.md"), "# skill\n");
await mkdir(join(home, ".escode", "cli"), { recursive: true });
await writeFile(
  join(home, ".escode", "cli", "config.json"),
  JSON.stringify({
    features: { mcp: true },
    plugins: {
      dirs: [alpha, beta],
      enabledPlugins: { "beta@inline": false },
    },
  }),
);
await freezeMtimes(alpha);
await freezeMtimes(beta);

const originalHome = process.env.HOME;
const originalUserProfile = process.env.USERPROFILE;
process.env.HOME = home;
process.env.USERPROFILE = home;

const golden = await (async () => {
  const service = createPluginSyncService();
  const candidates = await service.listLocalUserPluginCandidates();
  const target = {
    plugins: [
      { pluginId: "gamma@inline", directoryName: "remote-gamma" },
      { pluginId: "beta@inline", directoryName: "remote-beta" },
    ],
  };
  const statuses = await service.listRemoteUserPluginStatuses(target as never);
  const alphaCandidate = candidates.candidates.find((item) => item.name === "alpha")!;
  const exported = await service.exportPluginsArchive({ pluginIds: [alphaCandidate.id] });
  const access = await service.checkRemoteUserPluginWriteAccess();

  // 导入观察：Rust 侧会导入同一份归档到全新的 import-home；TS 在同样的空白状态录制。
  const importHome = join(root, "import-home");
  await mkdir(importHome, { recursive: true });
  process.env.HOME = importHome;
  process.env.USERPROFILE = importHome;
  const imported = await createPluginSyncService().importPluginsArchive({
    archive: new Uint8Array(exported.archive),
    overwrite: false,
  });
  const config = JSON.parse(
    await readFile(join(importHome, ".escode", "cli", "config.json"), "utf8"),
  ) as Record<string, unknown>;

  // gzip 容器随 zlib 实现不同，冻结解压后的 tar；metadata 导出时刻归一化后才是跨运行稳定字节。
  const archiveTar = normalizeMetadataMtime(gunzipSync(Buffer.from(exported.archive)));
  return sealLargeStrings({
    _readme:
      "rustHostPluginSyncParity.test.ts 的 TS createPluginSyncService 观察冻结（Phase 3 golden freeze）。由真实 TS 服务生成一次；删除 TS 后测试只加载本资产。绝对临时路径归一化为 $FIXTURE_ROOT；archiveTarBase64 是归一化 metadata mtime 后解压的 tar；超过 1 MiB 的字符串以 sha256 摘要存储。",
    candidates: normalize(candidates, root),
    statuses: normalize(statuses, root),
    export: {
      plugins: normalize(exported.plugins, root),
      archiveTarBase64: archiveTar.toString("base64"),
    },
    access: normalize(access, root),
    import: {
      result: normalize(imported, root),
      config: normalize(config, root),
    },
  });
})();

process.env.HOME = originalHome;
if (originalUserProfile === undefined) delete process.env.USERPROFILE;
else process.env.USERPROFILE = originalUserProfile;

const goldenDir = "packages/desktop/test/fixtures/golden";
const jsonPath = join(goldenDir, "rustHostPluginSyncParity.oracle.json");
const json = `${JSON.stringify(golden, null, 2)}\n`;
if (process.argv.includes("--check")) {
  deepEqual(
    JSON.parse(await readFile(jsonPath, "utf8")),
    JSON.parse(json),
    "rustHostPluginSyncParity golden 与当前 TS 观察漂移",
  );
  console.log("rustHostPluginSyncParity golden: OK");
} else {
  await mkdir(goldenDir, { recursive: true });
  await writeFile(jsonPath, json);
  console.log("wrote", jsonPath, "root=", root);
}
await rm(FIXTURE_ROOT, { recursive: true, force: true });
