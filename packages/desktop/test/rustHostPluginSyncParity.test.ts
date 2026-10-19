//! plugin-sync 差分：真实 TS `createPluginSyncService()` 作 oracle，经真实 `@escode/client` 调 Rust Host。
import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { gunzipSync } from "node:zlib";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createPluginSyncService } from "@escode/services/node";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

const originalHome = process.env.HOME;
const originalUserProfile = process.env.USERPROFILE;

/** metadata 条目的 mtime 是导出时刻（TS `Date.now()`）；归一 mtime+checksum 后其余 tar 字节必须全等。 */
function normalizeMetadataMtime(tar: Buffer): Buffer {
  const copy = Buffer.from(tar);
  const marker = Buffer.from(".escode-plugin-sync.json");
  const start = copy.indexOf(marker);
  if (start >= 0) {
    copy.fill(0, start + 136, start + 148);
    copy.fill(0, start + 148, start + 156);
  }
  return copy;
}

function plugin(root: string, name: string) {
  return mkdir(join(root, ".escode-plugin"), { recursive: true })
    .then(() =>
      writeFile(
        join(root, ".escode-plugin", "plugin.json"),
        JSON.stringify({ name, version: "1.0.0" }),
      ),
    )
    .then(() => writeFile(join(root, "README.md"), "# readme\n"));
}

describe.skipIf(!existsSync(hostBinary))("rust host plugin-sync parity", () => {
  it("matches candidates, statuses and archive bytes", async () => {
    const root = await mkdtemp(join(tmpdir(), "escode-rust-plugin-sync-"));
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

    process.env.HOME = home;
    process.env.USERPROFILE = home;
    const ts = createPluginSyncService();
    const host = startHost(home);
    try {
      await nextMeaningful(host);
      host.send({ kind: "rpcListen", bind: "127.0.0.1:0", home, dataBaseDir });
      const listening = await nextMeaningful(host);
      const services = await connectRpc(String(listening.addr), String(listening.secret));

      const tsCandidates = await ts.listLocalUserPluginCandidates();
      const rustCandidates = await services.pluginSyncService.listLocalUserPluginCandidates();
      expect(rustCandidates).toEqual(tsCandidates);

      const target = {
        plugins: [
          { pluginId: "gamma@inline", directoryName: "remote-gamma" },
          { pluginId: "beta@inline", directoryName: "remote-beta" },
        ],
      };
      expect(
        await services.pluginSyncService.listRemoteUserPluginStatuses(target as never),
      ).toEqual(await ts.listRemoteUserPluginStatuses(target));

      const alphaCandidate = tsCandidates.candidates.find((item) => item.name === "alpha")!;
      const tsExport = await ts.exportPluginsArchive({ pluginIds: [alphaCandidate.id] });
      const rustExport = await services.pluginSyncService.exportPluginsArchive({
        pluginIds: [alphaCandidate.id],
      } as never);
      // gzip 容器由 Node zlib / flate2 各自实现；格式对拍以解压后的 tar 为准。
      const rustTar = normalizeMetadataMtime(gunzipSync(Buffer.from(rustExport.archive)));
      const tsTar = normalizeMetadataMtime(gunzipSync(Buffer.from(tsExport.archive)));
      expect(rustTar.equals(tsTar)).toBe(true);
      expect(rustExport.archiveBytes).toBe(Buffer.from(rustExport.archive).length);
      expect(rustExport.plugins).toEqual(tsExport.plugins);

      const access = await services.pluginSyncService.checkRemoteUserPluginWriteAccess();
      expect(access.ok).toBe(true);
      expect(await ts.checkRemoteUserPluginWriteAccess()).toEqual(access);

      // 导入差分：Rust 导入到 importHome，记录结果与 config；清空后 TS 导入同一归档。
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
        const rustImport = await importServices.pluginSyncService.importPluginsArchive({
          archive: new Uint8Array(tsExport.archive),
          overwrite: false,
        } as never);
        const rustConfig = JSON.parse(
          await readFile(join(importHome, ".escode", "cli", "config.json"), "utf8"),
        );
        await rm(join(importHome, ".escode", "plugins"), { recursive: true, force: true });
        await rm(join(importHome, ".escode", "cli", "config.json"), { force: true });
        process.env.HOME = importHome;
        process.env.USERPROFILE = importHome;
        const tsImport = await createPluginSyncService().importPluginsArchive({
          archive: new Uint8Array(tsExport.archive),
          overwrite: false,
        });
        const tsConfig = JSON.parse(
          await readFile(join(importHome, ".escode", "cli", "config.json"), "utf8"),
        );
        expect(rustImport).toEqual(tsImport);
        expect(rustConfig).toEqual(tsConfig);
      } finally {
        await importHost.stop();
      }
    } finally {
      process.env.HOME = originalHome;
      if (originalUserProfile === undefined) delete process.env.USERPROFILE;
      else process.env.USERPROFILE = originalUserProfile;
      await host.stop();
      await rm(root, { recursive: true, force: true });
    }
  }, 300_000);
});
