import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, readFile, readdir, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

// docs/specs/rust-u2-user-config-services.md §4：TS `createSettingService` 对
// ensureDefaultProject / updateDataBaseDir 的观察已冻结为仓内资产
// （packages/desktop/test/fixtures/golden/rustHostSettingExtra.oracle.json）。
const goldenFile = new URL("./fixtures/golden/rustHostSettingExtra.oracle.json", import.meta.url);

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

function normalize(value: unknown, home: string, target: string): unknown {
  return JSON.parse(
    JSON.stringify(value ?? null)
      .split(home)
      .join("$FIXTURE_ROOT/home")
      .split(target)
      .join("$FIXTURE_ROOT/target"),
  );
}

describe("rust host setting extras", () => {
  it.skipIf(!existsSync(hostBinary))(
    "ensureDefaultProject and updateDataBaseDir match the frozen TS observations",
    async () => {
      const golden = JSON.parse(await readFile(goldenFile, "utf8")) as {
        ensureDefaultProject: Array<{ label: string; result: unknown }>;
        update: {
          result: unknown;
          tree: Record<string, string>;
          settingDataBaseDir: string;
        };
        reset: { result: unknown; hasDataBaseDir: boolean };
      };
      const base = await realpath(await mkdtemp(join(tmpdir(), "escode-rust-host-setting-")));
      const rustHome = join(base, "home");
      const rustTarget = join(base, "target");
      await mkdir(join(rustHome, ".escode", "v2", "sub"), { recursive: true });
      await writeFile(join(rustHome, ".escode", "v2", "a.json"), "old-a");
      await writeFile(join(rustHome, ".escode", "v2", "sub", "b.txt"), "b");
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

        // ensureDefaultProject
        for (const step of golden.ensureDefaultProject) {
          const actual = await outcome(() => rust.ensureDefaultProject(rustHome));
          expect({ label: step.label, v: normalize(actual, rustHome, rustTarget) }).toEqual({
            label: step.label,
            v: step.result,
          });
        }

        // updateDataBaseDir：迁移、不覆盖已有、设置落盘
        await mkdir(join(rustTarget, ".escode", "v2"), { recursive: true });
        await writeFile(join(rustTarget, ".escode", "v2", "a.json"), "keep-me");
        const actual = await outcome(() => rust.updateDataBaseDir(rustTarget));
        expect(normalize(actual, rustHome, rustTarget)).toEqual(golden.update.result);
        // tasks-index.sqlite 是 Rust Host 运行时索引缓存，不属于用户数据迁移的比对范围。
        const comparable = (value: Record<string, string>) =>
          Object.fromEntries(
            Object.entries(value).filter(([path]) => !path.endsWith("tasks-index.sqlite")),
          );
        expect(
          normalize(comparable(await tree(join(rustTarget, ".escode"))), rustHome, rustTarget),
        ).toEqual(golden.update.tree);
        const settingOf = async (home: string) =>
          JSON.parse(await readFile(join(home, ".escode", "v2", "setting.json"), "utf8")) as Record<
            string,
            unknown
          >;
        expect(normalize((await settingOf(rustHome)).dataBaseDir, rustHome, rustTarget)).toBe(
          golden.update.settingDataBaseDir,
        );
        expect((await settingOf(rustHome)).dataBaseDir).toBe(rustTarget);

        // 还原为默认：undefined → 键被移除
        const actualReset = await outcome(() => rust.updateDataBaseDir(undefined));
        expect(normalize(actualReset, rustHome, rustTarget)).toEqual(golden.reset.result);
        expect(Object.hasOwn(await settingOf(rustHome), "dataBaseDir")).toBe(
          golden.reset.hasDataBaseDir,
        );
      } finally {
        await host.stop();
        await rm(base, { recursive: true, force: true });
      }
    },
    120_000,
  );
});
