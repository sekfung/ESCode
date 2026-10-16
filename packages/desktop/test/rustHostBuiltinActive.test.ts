import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { copyFile, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import {
  resolveESCodeBuiltinCachePaths,
  resolveESCodeBuiltinClientPlatform,
} from "@escode/provider-node";
import { createProviderRuntime } from "@escode/services/node";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

// docs/specs/rust-u2-providers.md §7（P5a）：同一 Bundled/Active 夹具分别交给真实 TS Built-in Source 与 Rust Host，
// `model-selection.getView` 全等，且两端物化出的 Active 文件语义相同。

const BUILTIN_SOURCE = resolve("config/provider/escode-builtin.json");
const ENDPOINT = "https://api.example.com";
const APP_VERSION = "1.2.3";

type Release = { schemaVersion: 1; revision: number; config: Record<string, any> };

/** 取真实 Built-in 的变体：抬高 revision 并移除第一条 Provider 规则，视图因此可区分。 */
function variant(source: Release, revision: number, dropFirstProvider: boolean): Release {
  const copy = JSON.parse(JSON.stringify(source)) as Release;
  copy.revision = revision;
  if (dropFirstProvider) copy.config.providerConfigRules.providerRules.shift();
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

describe("rust host providers P5a", () => {
  it.skipIf(!existsSync(hostBinary) || !existsSync(BUILTIN_SOURCE))(
    "selects and materializes Built-in Active like the TS source",
    async () => {
      const base = await realpath(await mkdtemp(join(tmpdir(), "escode-rust-host-active-")));
      const bundledText = await readFile(BUILTIN_SOURCE, "utf8");
      const bundled = JSON.parse(bundledText) as Release;
      const builtin = join(base, "escode-builtin.json");
      await copyFile(BUILTIN_SOURCE, builtin);
      const personal = join(base, "provider_config.json");
      const home = join(base, "home");
      const tsRoot = join(base, "ts-cache");
      const rustRoot = join(base, "rust-cache");
      const platform = resolveESCodeBuiltinClientPlatform();
      const activeOf = (root: string) =>
        resolveESCodeBuiltinCachePaths({
          environmentConfigRoot: root,
          platform,
          appVersion: APP_VERSION,
          escodeEndpointOrigin: ENDPOINT,
        }).activeFilePath;
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

        for (const [label, activeContent] of SCENARIOS) {
          for (const root of [tsRoot, rustRoot]) {
            await rm(activeOf(root), { force: true });
            if (activeContent(bundled) !== null) {
              await mkdir(dirname(activeOf(root)), { recursive: true });
              await writeFile(activeOf(root), activeContent(bundled)!);
            }
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
            const strip = (view: { revision: number }) => ({ ...view, revision: "<n>" });
            const expected = await runtime.modelSelection.getView(undefined);
            const actual = await services.modelSelectionService.getView(undefined);
            expect({ label, view: strip(actual) }).toEqual({
              label,
              view: strip(JSON.parse(JSON.stringify(expected))),
            });
            const tsActive = JSON.parse(await readFile(activeOf(tsRoot), "utf8"));
            const rustActive = JSON.parse(await readFile(activeOf(rustRoot), "utf8"));
            expect({ label, active: rustActive }).toEqual({ label, active: tsActive });
          } finally {
            runtime.dispose();
          }
        }
      } finally {
        await host.stop();
        await rm(base, { recursive: true, force: true });
      }
    },
    120_000,
  );
});
