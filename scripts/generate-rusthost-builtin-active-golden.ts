// Phase 3 golden freeze：rustHostBuiltinActive.test.ts 的 TS `createProviderRuntime` 观察录制。
// 一次性脚本；删除 TS 实现前用于重录。`--check` 校验仓内 golden 与当前 TS 观察一致。
//
// 归一化：临时根路径写成 `$FIXTURE_ROOT`（测试运行时替换）；Registry revision 是进程内单调计数，
// 统一写成 `<n>`，与测试中的 `strip` 保持一致。
import { deepEqual } from "node:assert/strict";
import { copyFile, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { gzipSync, gunzipSync } from "node:zlib";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import {
  resolveESCodeBuiltinCachePaths,
  resolveESCodeBuiltinClientPlatform,
} from "../packages/provider-node/src/index.ts";
import { createProviderRuntime } from "../packages/services/src/node.ts";

const BUILTIN_SOURCE = resolve("config/provider/escode-builtin.json");
const ENDPOINT = "https://api.example.com";
const APP_VERSION = "1.2.3";

type Release = { schemaVersion: 1; revision: number; config: Record<string, any> };

/** 取真实 Built-in 的变体：设定 revision 并移除第一条模板规则（`providerTemplates` 进入 settings 视图，视图因此可区分）。 */
function variant(source: Release, revision: number, dropFirstProvider: boolean): Release {
  const copy = JSON.parse(JSON.stringify(source)) as Release;
  copy.revision = revision;
  if (dropFirstProvider) copy.config.providerConfigRules.templateRules.shift();
  return copy;
}

const SCENARIOS: Array<[string, (bundled: Release) => string | null]> = [
  ["no active file", () => null],
  ["newer active", (b) => JSON.stringify(variant(b, b.revision + 5, true))],
  ["older active", (b) => JSON.stringify(variant(b, Math.max(0, b.revision - 1), true))],
  ["same revision conflict", (b) => JSON.stringify(variant(b, b.revision, true))],
  ["same revision same content", (b) => JSON.stringify(b)],
  ["corrupt active", () => "{not json"],
  ["schema-invalid active", () => JSON.stringify({ schemaVersion: 1, revision: 99, config: {} })],
];

/** 与测试一致：Registry revision 是进程内单调计数，统一写成 `<n>`。 */
const strip = (view: { revision: number }) => ({ ...view, revision: "<n>" });
/** 绝对路径归一化：测试运行时把 `$FIXTURE_ROOT` 换成同一工作根。 */
const normalize = (value: unknown, root: string): unknown =>
  JSON.parse(JSON.stringify(value).split(root).join("$FIXTURE_ROOT"));

const base = await realpath(await mkdtemp(join(tmpdir(), "escode-rust-host-active-golden-")));
const bundledText = await readFile(BUILTIN_SOURCE, "utf8");
const bundled = JSON.parse(bundledText) as Release;
const builtin = join(base, "escode-builtin.json");
await copyFile(BUILTIN_SOURCE, builtin);
const personal = join(base, "provider_config.json");
const tsRoot = join(base, "ts-cache");
const platform = resolveESCodeBuiltinClientPlatform();
const activeOf = (root: string) =>
  resolveESCodeBuiltinCachePaths({
    environmentConfigRoot: root,
    platform,
    appVersion: APP_VERSION,
    escodeEndpointOrigin: ENDPOINT,
  }).activeFilePath;

const scenarios: Array<Record<string, unknown>> = [];
try {
  for (const [label, activeContent] of SCENARIOS) {
    await rm(activeOf(tsRoot), { force: true });
    if (activeContent(bundled) !== null) {
      await mkdir(dirname(activeOf(tsRoot)), { recursive: true });
      await writeFile(activeOf(tsRoot), activeContent(bundled)!);
    }
    const runtime = createProviderRuntime({
      escodeBuiltinFilePath: builtin,
      escodeBuiltinEnvironment: {
        environmentConfigRoot: tsRoot,
        platform,
        appVersion: APP_VERSION,
        resolveEndpointOrigin: () => ENDPOINT,
        fetchRelease: async () => null,
        watch: false,
      },
      personalFilePath: personal,
      watch: false,
      personalPollingIntervalMs: false,
    });
    try {
      const view = strip(
        JSON.parse(JSON.stringify(await runtime.providerSettings.getView())) as {
          revision: number;
        },
      );
      const active = JSON.parse(await readFile(activeOf(tsRoot), "utf8"));
      scenarios.push({
        label,
        view: normalize(view, base),
        active: normalize(active, base),
      });
    } finally {
      runtime.dispose();
    }
  }
} finally {
  await rm(base, { recursive: true, force: true });
}

const golden = {
  _readme:
    "rustHostBuiltinActive.test.ts 的 TS createProviderRuntime 观察冻结（Phase 3 golden freeze）。revision 归一化为 <n>；绝对临时路径归一化为 $FIXTURE_ROOT，测试运行时替换为本次工作根。",
  scenarios,
};
const goldenDir = "packages/desktop/test/fixtures/golden";
const json = `${JSON.stringify(golden, null, 2)}\n`;
const compressed = Buffer.byteLength(json) > 1_000_000;
const goldenPath = join(goldenDir, `rustHostBuiltinActive.oracle.json${compressed ? ".gz" : ""}`);
if (process.argv.includes("--check")) {
  const committed = compressed
    ? JSON.parse(gunzipSync(await readFile(goldenPath)).toString("utf8"))
    : JSON.parse(await readFile(goldenPath, "utf8"));
  deepEqual(committed, JSON.parse(json), "rustHostBuiltinActive golden 与当前 TS 观察漂移");
  console.log("rustHostBuiltinActive golden: OK");
} else {
  await mkdir(goldenDir, { recursive: true });
  await writeFile(goldenPath, compressed ? gzipSync(Buffer.from(json)) : json);
  console.log("wrote", goldenPath, compressed ? "(gzip)" : "");
}
