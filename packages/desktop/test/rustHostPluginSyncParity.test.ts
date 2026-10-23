// Phase 3 golden freeze：TS `createPluginSyncService` 的候选/状态/归档/导入观察已冻结为仓内资产
// （packages/desktop/test/fixtures/golden/）。测试不再实例化 TS 服务，删除 TS 实现后仍能独立重验 Rust Host。
import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { createHash } from "node:crypto";
import { gunzipSync, gzipSync } from "node:zlib";
import { mkdir, readFile, readdir, realpath, rm, utimes, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

const goldenFile = new URL(
  "./fixtures/golden/rustHostPluginSyncParity.oracle.json",
  import.meta.url,
);

// 候选 id 是 `${pluginId}:${canonicalPath}` 的 sha256，路径不同会得到不同 id；因此夹具必须使用
// 固定根目录（与 rustHostFileIndex 一致）。冻结资产里的 `$FIXTURE_ROOT` 在运行时替换为同一根目录。
const FIXTURE_ROOT = "/tmp/escode-rust-plugin-sync-fixture";
const FIXED_MTIME_SECONDS = 1_600_000_000;

interface Golden {
  candidates: {
    candidates: Array<{ id: string; name: string }>;
    maxArchiveBytes: number;
  };
  statuses: unknown;
  export: { plugins: unknown; archiveTarBase64: string };
  access: unknown;
  import: { result: unknown; config: unknown };
}

async function plugin(root: string, name: string): Promise<void> {
  await mkdir(join(root, ".escode-plugin"), { recursive: true });
  await writeFile(
    join(root, ".escode-plugin", "plugin.json"),
    JSON.stringify({ name, version: "1.0.0" }),
  );
  await writeFile(join(root, "README.md"), "# readme\n");
}

/** 递归固定 mtime（先子后父）：tar 头按秒记录源 mtime，必须与录制时一致。 */
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
    for (let index = start; index < start + 512; index += 1) checksum += copy[index] ?? 0;
    copy.write(checksum.toString(8).padStart(6, "0"), start + 148, 6, "ascii");
    copy[start + 154] = 0;
    copy[start + 155] = 0x20;
  }
  return copy;
}

/** 冻结资产对 >1 MiB 的字符串只保留 sha256；两侧同法归一后再比较。 */
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

describe.skipIf(!existsSync(hostBinary))("rust host plugin-sync parity", () => {
  it("matches candidates, statuses and archive bytes", async () => {
    await rm(FIXTURE_ROOT, { recursive: true, force: true });
    await mkdir(FIXTURE_ROOT, { recursive: true });
    // /tmp 在部分平台是软链；candidate.path / id 使用 canonical root，与录制脚本保持一致。
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

    const golden = JSON.parse(
      (await readFile(goldenFile, "utf8")).split("$FIXTURE_ROOT").join(root),
    ) as Golden;

    const host = startHost(home);
    try {
      await nextMeaningful(host);
      host.send({ kind: "rpcListen", bind: "127.0.0.1:0", home, dataBaseDir });
      const listening = await nextMeaningful(host);
      const services = await connectRpc(String(listening.addr), String(listening.secret));

      const rustCandidates = await services.pluginSyncService.listLocalUserPluginCandidates();
      expect(sealLargeStrings(rustCandidates)).toEqual(golden.candidates);

      const target = {
        plugins: [
          { pluginId: "gamma@inline", directoryName: "remote-gamma" },
          { pluginId: "beta@inline", directoryName: "remote-beta" },
        ],
      };
      const rustStatuses = await services.pluginSyncService.listRemoteUserPluginStatuses(
        target as never,
      );
      expect(sealLargeStrings(rustStatuses)).toEqual(golden.statuses);

      const alphaCandidate = golden.candidates.candidates.find((item) => item.name === "alpha")!;
      const rustExport = await services.pluginSyncService.exportPluginsArchive({
        pluginIds: [alphaCandidate.id],
      } as never);
      // gzip 容器由 Node zlib / flate2 各自实现；格式对拍以解压后的 tar 为准。
      const rustTar = normalizeMetadataMtime(gunzipSync(Buffer.from(rustExport.archive)));
      expect(
        sealLargeStrings({
          plugins: rustExport.plugins,
          archiveTarBase64: rustTar.toString("base64"),
        }),
      ).toEqual(golden.export);
      expect(rustExport.archiveBytes).toBe(Buffer.from(rustExport.archive).length);

      const access = await services.pluginSyncService.checkRemoteUserPluginWriteAccess();
      expect(access.ok).toBe(true);
      expect(sealLargeStrings(access)).toEqual(golden.access);

      // 导入差分：Rust 导入冻结的 TS 归档到 import-home，结果与 config 对冻结观察。
      const importHome = join(root, "import-home");
      await mkdir(importHome, { recursive: true });
      const importHost = startHost(importHome);
      try {
        await nextMeaningful(importHost);
        importHost.send({ kind: "rpcListen", bind: "127.0.0.1:0", home: importHome, dataBaseDir });
        const importListening = await nextMeaningful(importHost);
        const importServices = await connectRpc(
          String(importListening.addr),
          String(importListening.secret),
        );
        if (golden.export.archiveTarBase64.startsWith("sha256:")) {
          throw new Error("冻结归档超过 1 MiB 只保留了 sha256，无法驱动导入场景");
        }
        const archive = gzipSync(Buffer.from(golden.export.archiveTarBase64, "base64"));
        const rustImport = await importServices.pluginSyncService.importPluginsArchive({
          archive: new Uint8Array(archive),
          overwrite: false,
        } as never);
        const rustConfig = JSON.parse(
          await readFile(join(importHome, ".escode", "cli", "config.json"), "utf8"),
        );
        expect(sealLargeStrings(rustImport)).toEqual(golden.import.result);
        expect(sealLargeStrings(rustConfig)).toEqual(golden.import.config);
      } finally {
        await importHost.stop();
      }
    } finally {
      await host.stop();
      await rm(FIXTURE_ROOT, { recursive: true, force: true });
    }
  }, 300_000);
});
