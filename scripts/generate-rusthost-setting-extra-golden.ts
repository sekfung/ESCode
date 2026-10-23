// Phase 3 golden freeze：rustHostSettingExtra.test.ts 的 TS `createSettingService` 观察录制。
// 一次性脚本；删除 TS 实现前用于重录。`--check` 校验仓内 golden 与当前 TS 观察一致。
//
// 归一化：TS home 写成 `$FIXTURE_ROOT/home`，迁移目标写成 `$FIXTURE_ROOT/target`，测试运行时使用同一相对布局。
import { deepEqual } from "node:assert/strict";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, readFile, readdir, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

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

const base = await realpath(await mkdtemp(join(tmpdir(), "rusthost-setting-extra-golden-")));
const tsHome = join(base, "ts-home");
const tsTarget = join(base, "ts-target");
const previousHome = process.env.HOME;
process.env.HOME = tsHome;

try {
  for (const home of [tsHome]) {
    await mkdir(join(home, ".escode", "v2", "sub"), { recursive: true });
    await writeFile(join(home, ".escode", "v2", "a.json"), "old-a");
    await writeFile(join(home, ".escode", "v2", "sub", "b.txt"), "b");
  }
  const { createSettingService } = await import("../packages/services/src/node.ts");
  const ts = createSettingService();

  const ensureDefaultProject = [];
  for (const label of ["first", "second"]) {
    const value = await outcome(() => ts.ensureDefaultProject(tsHome));
    ensureDefaultProject.push({ label, result: normalize(value, tsHome, tsTarget) });
  }

  await mkdir(join(tsTarget, ".escode", "v2"), { recursive: true });
  await writeFile(join(tsTarget, ".escode", "v2", "a.json"), "keep-me");
  const updateOutcome = await outcome(() => ts.updateDataBaseDir(tsTarget));
  // tasks-index.sqlite 是 Rust Host 运行时索引缓存，不属于用户数据迁移的比对范围。
  const comparable = (value: Record<string, string>) =>
    Object.fromEntries(
      Object.entries(value).filter(([path]) => !path.endsWith("tasks-index.sqlite")),
    );
  const updateTree = normalize(comparable(await tree(join(tsTarget, ".escode"))), tsHome, tsTarget);
  const settingOf = async (home: string) =>
    JSON.parse(await readFile(join(home, ".escode", "v2", "setting.json"), "utf8")) as Record<
      string,
      unknown
    >;
  const updatedSetting = await settingOf(tsHome);

  const resetOutcome = await outcome(() => ts.updateDataBaseDir(undefined));
  const resetSetting = await settingOf(tsHome);

  const golden = {
    _readme:
      "rustHostSettingExtra.test.ts 的 TS createSettingService 观察冻结（Phase 3 golden freeze）。TS home 归一化为 $FIXTURE_ROOT/home，迁移目标归一化为 $FIXTURE_ROOT/target。",
    ensureDefaultProject,
    update: {
      result: normalize(updateOutcome, tsHome, tsTarget),
      tree: updateTree,
      settingDataBaseDir: normalize(updatedSetting.dataBaseDir, tsHome, tsTarget),
    },
    reset: {
      result: normalize(resetOutcome, tsHome, tsTarget),
      hasDataBaseDir: Object.hasOwn(resetSetting, "dataBaseDir"),
    },
  };

  const goldenDir = "packages/desktop/test/fixtures/golden";
  const goldenPath = join(goldenDir, "rustHostSettingExtra.oracle.json");
  const json = `${JSON.stringify(golden, null, 2)}\n`;
  if (process.argv.includes("--check")) {
    deepEqual(
      JSON.parse(await readFile(goldenPath, "utf8")),
      JSON.parse(json),
      "rustHostSettingExtra golden 与当前 TS 观察漂移",
    );
    console.log("rustHostSettingExtra golden: OK");
  } else {
    await mkdir(goldenDir, { recursive: true });
    await writeFile(goldenPath, json);
    console.log("wrote", goldenPath, "home=", tsHome, "target=", tsTarget);
  }
} finally {
  if (previousHome === undefined) delete process.env.HOME;
  else process.env.HOME = previousHome;
  await rm(base, { recursive: true, force: true });
}
