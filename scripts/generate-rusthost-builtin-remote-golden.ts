// Phase 3 golden freeze：rustHostBuiltinRemote.test.ts 的 TS Synchronizer / ProviderRuntime 观察录制。
// 一次性脚本；删除 TS 实现前用于重录。`--check` 校验仓内 golden 与当前 TS 观察一致。
//
// 归一化：临时根路径写成 `$FIXTURE_ROOT`（测试运行时替换）；时间字段只保留语义
// （`nextEligibleAt` -> `<future>`）；Registry revision 统一写成 `<n>`。
import { deepEqual } from "node:assert/strict";
import { copyFile, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import {
  ESCodeBuiltinRemoteSynchronizer,
  NodeESCodeBuiltinProviderConfigSource,
  downloadESCodeBuiltinRelease,
  resolveESCodeBuiltinCachePaths,
  resolveESCodeBuiltinClientPlatform,
} from "../packages/provider-node/src/index.ts";
import { createProviderRuntime } from "../packages/services/src/node.ts";

const BUILTIN_SOURCE = resolve("config/provider/escode-builtin.json");
const APP_VERSION = "1.2.3";

type Release = { schemaVersion: 1; revision: number; config: Record<string, any> };

async function startStub(): Promise<{ origin: string; urls: string[]; server: Server }> {
  const urls: string[] = [];
  const server = createServer((request, response) => {
    urls.push(`${request.url}`);
    response.setHeader("content-type", "application/json");
    // 无 `builtin_provider_config_json` = Endpoint 没有配置 Release（Missing，非错误）。
    response.end(JSON.stringify({ code: 0, data: { configs: {} } }));
  });
  await new Promise<void>((done) => server.listen(0, "127.0.0.1", done));
  const { port } = server.address() as AddressInfo;
  return { origin: `http://127.0.0.1:${port}`, urls, server };
}

/** 与测试一致：Registry revision 是进程内单调计数，统一写成 `<n>`。 */
const strip = (view: { revision: number }) => ({ ...view, revision: "<n>" });
/** 绝对路径归一化：测试运行时把 `$FIXTURE_ROOT` 换成同一工作根。 */
const normalize = (value: unknown, root: string): unknown =>
  JSON.parse(JSON.stringify(value).split(root).join("$FIXTURE_ROOT"));
/** 控制文件时间字段只校验语义：成功收口 = 无租约、一小时后才可再刷。 */
const normalizeControl = (control: Record<string, any>, origin: string) => ({
  ...control,
  endpointKey: control.endpointKey === origin ? "<origin>" : "?",
  nextEligibleAt: "<future>",
});

const base = await realpath(await mkdtemp(join(tmpdir(), "escode-rust-host-remote-golden-")));
const bundledText = await readFile(BUILTIN_SOURCE, "utf8");
const bundled = JSON.parse(bundledText) as Release;
const builtin = join(base, "escode-builtin.json");
await copyFile(BUILTIN_SOURCE, builtin);
const personal = join(base, "provider_config.json");
const tsRoot = join(base, "ts-cache");
const platform = resolveESCodeBuiltinClientPlatform();
const pathsOf = (root: string, origin: string) =>
  resolveESCodeBuiltinCachePaths({
    environmentConfigRoot: root,
    platform,
    appVersion: APP_VERSION,
    escodeEndpointOrigin: origin,
  });
const stub = await startStub();

let golden: Record<string, unknown>;
try {
  // TS 同步器对本地桩做一次刷新，记录请求形状与控制文件收口。
  const tsPaths = pathsOf(tsRoot, stub.origin);
  const tsSource = new NodeESCodeBuiltinProviderConfigSource({
    bundledFilePath: builtin,
    activeFilePath: tsPaths.activeFilePath,
    watch: false,
  });
  const tsSync = new ESCodeBuiltinRemoteSynchronizer({
    source: tsSource,
    controlFilePath: tsPaths.controlFilePath,
    resolveEndpointKey: () => stub.origin,
    fetchRelease: (endpointOrigin, signal) =>
      downloadESCodeBuiltinRelease({
        endpointOrigin,
        appVersion: APP_VERSION,
        platform,
        signal,
        request: (url, init) => fetch(url, init),
      }),
  });
  const refreshResult = await tsSync.refresh();
  tsSync.dispose();
  tsSource.dispose();
  const control = JSON.parse(await readFile(tsPaths.controlFilePath, "utf8"));

  // Endpoint 切换：给 B 预置一份更新的 Active，记录 TS 在该缓存上的 settings 视图。
  const originB = "http://127.0.0.1:1";
  const seeded = JSON.parse(bundledText) as Release;
  seeded.revision = bundled.revision + 5;
  seeded.config.providerConfigRules.templateRules.shift();
  const tsSeededRoot = join(base, "ts-cache-b");
  const tsActiveB = pathsOf(tsSeededRoot, originB).activeFilePath;
  await mkdir(dirname(tsActiveB), { recursive: true });
  await writeFile(tsActiveB, JSON.stringify(seeded));
  const runtime = createProviderRuntime({
    escodeBuiltinFilePath: builtin,
    escodeBuiltinEnvironment: {
      environmentConfigRoot: tsSeededRoot,
      platform,
      appVersion: APP_VERSION,
      resolveEndpointOrigin: () => originB,
      fetchRelease: async () => null,
      watch: false,
    },
    personalFilePath: personal,
    watch: false,
    personalPollingIntervalMs: false,
  });
  let viewAfterSwitch: unknown;
  try {
    viewAfterSwitch = strip(
      JSON.parse(JSON.stringify(await runtime.providerSettings.getView())) as { revision: number },
    );
  } finally {
    runtime.dispose();
  }

  golden = {
    _readme:
      "rustHostBuiltinRemote.test.ts 的 TS Synchronizer / ProviderRuntime 观察冻结（Phase 3 golden freeze）。控制文件时间字段归一化为 <future>，endpointKey 归一化为 <origin>，revision 归一化为 <n>，请求路径的平台段归一化为 $PLATFORM；绝对临时路径归一化为 $FIXTURE_ROOT。",
    refreshResult,
    // 请求路径里的 platform 随录制机器变化；写占位符，测试运行时按当前平台替换。
    requestPath: stub.urls[0].split(platform).join("$PLATFORM"),
    control: normalizeControl(control, stub.origin),
    viewAfterSwitch: normalize(viewAfterSwitch, base),
  };
} finally {
  stub.server.close();
  await rm(base, { recursive: true, force: true });
}

const goldenDir = "packages/desktop/test/fixtures/golden";
const goldenPath = join(goldenDir, "rustHostBuiltinRemote.oracle.json");
const json = `${JSON.stringify(golden, null, 2)}\n`;
if (process.argv.includes("--check")) {
  deepEqual(
    JSON.parse(await readFile(goldenPath, "utf8")),
    JSON.parse(json),
    "rustHostBuiltinRemote golden 与当前 TS 观察漂移",
  );
  console.log("rustHostBuiltinRemote golden: OK");
} else {
  await mkdir(goldenDir, { recursive: true });
  await writeFile(goldenPath, json);
  console.log("wrote", goldenPath);
}
