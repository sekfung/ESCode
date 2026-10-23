import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { copyFile, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { gunzipSync } from "node:zlib";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import {
  resolveESCodeBuiltinCachePaths,
  resolveESCodeBuiltinClientPlatform,
} from "@escode/provider-node";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

// Phase 3 golden freeze：TS Built-in Source / ProviderRuntime 的 `providerSettings.getView`
// 与物化出的 Active 文件观察已冻结（packages/desktop/test/fixtures/golden/
// rustHostBuiltinActive.oracle.json.gz）。测试不再实例化 TS runtime，删除 TS 实现后仍能
// 独立重验 Rust Host 的 Bundled/Active 选择与物化语义。
const goldenFile = new URL(
  "./fixtures/golden/rustHostBuiltinActive.oracle.json.gz",
  import.meta.url,
);

// docs/specs/rust-u2-providers.md §11（P5a）：同一 Bundled/Active 夹具分别交给冻结的 TS 观察与
// Rust Host，`model-selection.getView` 全等，且两端物化出的 Active 文件语义相同。

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

interface BuiltinActiveGolden {
  scenarios: Array<{
    label: string;
    view: Record<string, unknown>;
    active: Record<string, unknown>;
  }>;
}

describe("rust host providers P5a", () => {
  it.skipIf(!existsSync(hostBinary) || !existsSync(BUILTIN_SOURCE))(
    "selects and materializes Built-in Active like the frozen TS source",
    async () => {
      const base = await realpath(await mkdtemp(join(tmpdir(), "escode-rust-host-active-")));
      const bundledText = await readFile(BUILTIN_SOURCE, "utf8");
      const bundled = JSON.parse(bundledText) as Release;
      const builtin = join(base, "escode-builtin.json");
      await copyFile(BUILTIN_SOURCE, builtin);
      const personal = join(base, "provider_config.json");
      const home = join(base, "home");
      const rustRoot = join(base, "rust-cache");
      const platform = resolveESCodeBuiltinClientPlatform();
      const activeOf = (root: string) =>
        resolveESCodeBuiltinCachePaths({
          environmentConfigRoot: root,
          platform,
          appVersion: APP_VERSION,
          escodeEndpointOrigin: ENDPOINT,
        }).activeFilePath;
      // 冻结观察中的绝对临时路径统一写成 $FIXTURE_ROOT；替换成本次运行的工作根后逐字段比较。
      const golden = JSON.parse(
        gunzipSync(await readFile(goldenFile))
          .toString("utf8")
          .split("$FIXTURE_ROOT")
          .join(base),
      ) as BuiltinActiveGolden;
      expect(golden.scenarios).toHaveLength(SCENARIOS.length);
      const host = startHost(home);
      try {
        expect((await nextMeaningful(host)).kind).toBe("hostReady");
        host.send({
          kind: "rpcListen",
          bind: "127.0.0.1:0",
          home,
          runtimeEnv: {
            ESCODE_BUILTIN_PROVIDER_CONFIG_FILE: builtin,
            ESCODE_PERSONAL_PROVIDER_CONFIG_FILE: personal,
            ESCODE_BUILTIN_CACHE_ROOT: rustRoot,
            ESCODE_APP_VERSION: APP_VERSION,
            ESCODE_ENDPOINT_ORIGIN: ENDPOINT,
          },
        });
        const listening = await nextMeaningful(host);
        const services = await connectRpc(String(listening.addr), String(listening.secret));

        const strip = (view: { revision: number }) => ({ ...view, revision: "<n>" });
        for (const [index, [label, activeContent]] of SCENARIOS.entries()) {
          const expected = golden.scenarios[index];
          expect(expected.label).toBe(label);
          await rm(activeOf(rustRoot), { force: true });
          if (activeContent(bundled) !== null) {
            await mkdir(dirname(activeOf(rustRoot)), { recursive: true });
            await writeFile(activeOf(rustRoot), activeContent(bundled)!);
          }
          const actual = await services.providerSettingsService.getView();
          expect({ label, view: strip(actual) }).toEqual({ label, view: expected.view });
          const rustActive = JSON.parse(await readFile(activeOf(rustRoot), "utf8"));
          expect({ label, active: rustActive }).toEqual({ label, active: expected.active });
        }
      } finally {
        await host.stop();
        await rm(base, { recursive: true, force: true });
      }
    },
    120_000,
  );
});
