import { describe, expect, it, vi } from "vitest";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, readFile, readdir, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

// docs/specs/rust-u2-user-config-services.md §4：setting.ensureDefaultProject / updateDataBaseDir 对真实 TS 设置服务差分。

async function tree(root: string): Promise<Record<string, string>> {
  const result: Record<string, string> = {};
  const walk = async (dir: string) => {
    if (!existsSync(dir)) return;
    for (const entry of await readdir(dir, { withFileTypes: true })) {
      const path = join(dir, entry.name);
      if (entry.isDirectory()) await walk(path);
      else result[path.slice(root.length)] = await readFile(path, "utf8");
    }
  };
  await walk(root);
  return result;
}

async function outcome<T>(run: () => Promise<T>) {
  try {
    return { ok: true as const, value: JSON.parse(JSON.stringify((await run()) ?? null)) };
  } catch (error) {
    return { ok: false as const, message: (error as Error).message };
  }
}

describe("rust host setting extras", () => {
  it.skipIf(!existsSync(hostBinary))(
    "ensureDefaultProject and updateDataBaseDir match the TS setting service",
    async () => {
      const base = await realpath(await mkdtemp(join(tmpdir(), "escode-rust-host-setting-")));
      const tsHome = join(base, "ts-home");
      const rustHome = join(base, "rust-home");
      const previousHome = process.env.HOME;
      for (const home of [tsHome, rustHome]) {
        await mkdir(join(home, ".escode", "v2", "sub"), { recursive: true });
        await writeFile(join(home, ".escode", "v2", "a.json"), "old-a");
        await writeFile(join(home, ".escode", "v2", "sub", "b.txt"), "b");
      }
      process.env.HOME = tsHome;
      vi.resetModules();
      const { createSettingService } = await import("@escode/services/node");
      const ts = createSettingService();
      const host = startHost(rustHome);
      try {
        expect((await nextMeaningful(host)).kind).toBe("hostReady");
        host.send({
          kind: "rpcListen",
          bind: "127.0.0.1:0",
          home: rustHome,
          dataBaseDir: rustHome,
        });
        const listening = await nextMeaningful(host);
        const rust = (await connectRpc(String(listening.addr), String(listening.secret)))
          .settingService;

        const norm = (value: unknown, home: string) =>
          JSON.parse(
            JSON.stringify(value ?? null)
              .split(home)
              .join("<home>"),
          );
        // ensureDefaultProject
        for (const label of ["first", "second"]) {
          const expected = await outcome(() => ts.ensureDefaultProject(tsHome));
          const actual = await outcome(() => rust.ensureDefaultProject(rustHome));
          expect({ label, v: norm(actual, rustHome) }).toEqual({
            label,
            v: norm(expected, tsHome),
          });
        }

        // updateDataBaseDir：迁移、不覆盖已有、设置落盘
        const tsTarget = join(base, "ts-target");
        const rustTarget = join(base, "rust-target");
        for (const target of [tsTarget, rustTarget]) {
          await mkdir(join(target, ".escode", "v2"), { recursive: true });
          await writeFile(join(target, ".escode", "v2", "a.json"), "keep-me");
        }
        const expected = await outcome(() => ts.updateDataBaseDir(tsTarget));
        const actual = await outcome(() => rust.updateDataBaseDir(rustTarget));
        expect(actual).toEqual(expected);
        // tasks-index.sqlite 是 Rust Host 运行时索引缓存，不属于用户数据迁移的比对范围。
        const comparable = (value: Record<string, string>) =>
          Object.fromEntries(
            Object.entries(value).filter(([path]) => !path.endsWith("tasks-index.sqlite")),
          );
        expect(norm(comparable(await tree(join(rustTarget, ".escode"))), rustTarget)).toEqual(
          norm(comparable(await tree(join(tsTarget, ".escode"))), tsTarget),
        );
        const settingOf = async (home: string) =>
          JSON.parse(await readFile(join(home, ".escode", "v2", "setting.json"), "utf8"));
        expect((await settingOf(rustHome)).dataBaseDir).toBe(rustTarget);
        expect((await settingOf(tsHome)).dataBaseDir).toBe(tsTarget);

        // 还原为默认：undefined → 键被移除
        const expectedReset = await outcome(() => ts.updateDataBaseDir(undefined));
        const actualReset = await outcome(() => rust.updateDataBaseDir(undefined));
        expect(actualReset.ok).toBe(expectedReset.ok);
        expect("dataBaseDir" in (await settingOf(rustHome))).toBe(
          "dataBaseDir" in (await settingOf(tsHome)),
        );
      } finally {
        if (previousHome === undefined) delete process.env.HOME;
        else process.env.HOME = previousHome;
        await host.stop();
        await rm(base, { recursive: true, force: true });
      }
    },
    120_000,
  );
});
