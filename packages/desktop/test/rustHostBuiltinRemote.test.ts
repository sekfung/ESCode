import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
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
} from "@escode/provider-node";
import { createProviderRuntime } from "@escode/services/node";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

// docs/specs/rust-u2-providers.md §12（P5b）：本地 HTTP 桩作 Endpoint，Rust Host 与真实 TS 同步器对同一桩刷新，
// 请求形状与控制文件一致；设置里切换 `escodeEndpointOrigin` 后 Host 的视图随之切到新 Endpoint 的缓存。

const BUILTIN_SOURCE = resolve("config/provider/escode-builtin.json");
const APP_VERSION = "1.2.3";

type Release = { schemaVersion: 1; revision: number; config: Record<string, any> };

async function startStub(): Promise<{ origin: string; urls: string[]; server: Server }> {
  const urls: string[] = [];
  const server = createServer((request, response) => {
    urls.push(`${request.url}`);
    // 不带凭据：Host 的下载请求没有 Authorization/Cookie。
    expect(request.headers.authorization).toBeUndefined();
    expect(request.headers.cookie).toBeUndefined();
    response.setHeader("content-type", "application/json");
    // 无 `builtin_provider_config_json` = Endpoint 没有配置 Release（Missing，非错误）。
    response.end(JSON.stringify({ code: 0, data: { configs: {} } }));
  });
  await new Promise<void>((done) => server.listen(0, "127.0.0.1", done));
  const { port } = server.address() as AddressInfo;
  return { origin: `http://127.0.0.1:${port}`, urls, server };
}

async function waitFor(predicate: () => boolean | Promise<boolean>, what: string) {
  // 按时间而非迭代次数：失败路径上每次探测本身也要耗时，测试必须在截止时间内快速失败。
  const deadline = Date.now() + 8_000;
  while (Date.now() < deadline) {
    if (await predicate()) return;
    await new Promise((done) => setTimeout(done, 20));
  }
  throw new Error(`timed out waiting for ${what}`);
}

describe("rust host providers P5b", () => {
  it.skipIf(!existsSync(hostBinary) || !existsSync(BUILTIN_SOURCE))(
    "refreshes like the TS synchronizer and follows the settings endpoint override",
    async () => {
      const base = await realpath(await mkdtemp(join(tmpdir(), "escode-rust-host-remote-")));
      const bundledText = await readFile(BUILTIN_SOURCE, "utf8");
      const bundled = JSON.parse(bundledText) as Release;
      const builtin = join(base, "escode-builtin.json");
      await copyFile(BUILTIN_SOURCE, builtin);
      const personal = join(base, "provider_config.json");
      const home = join(base, "home");
      const rustRoot = join(base, "rust-cache");
      const tsRoot = join(base, "ts-cache");
      const platform = resolveESCodeBuiltinClientPlatform();
      const pathsOf = (root: string, origin: string) =>
        resolveESCodeBuiltinCachePaths({
          environmentConfigRoot: root,
          platform,
          appVersion: APP_VERSION,
          escodeEndpointOrigin: origin,
        });
      const stubA = await startStub();
      const stubTs = await startStub();
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
            ESCODE_ENV: "test",
            ESCODE_ENDPOINT_ORIGIN: stubA.origin,
            ESCODE_BUILTIN_REMOTE_REFRESH: "1",
          },
        });
        const listening = await nextMeaningful(host);
        const services = await connectRpc(String(listening.addr), String(listening.secret));
        // 首次读取启动周期刷新（立即一次）。
        await services.modelSelectionService.getView(undefined);
        const rustControl = pathsOf(rustRoot, stubA.origin).controlFilePath;
        await waitFor(() => existsSync(rustControl), "Rust Host's refresh control file");
        await waitFor(() => stubA.urls.length >= 1, "Rust Host's client-config request");

        // TS 同步器对另一份缓存、另一个桩做同样的一次刷新。
        const tsPaths = pathsOf(tsRoot, stubTs.origin);
        const tsSource = new NodeESCodeBuiltinProviderConfigSource({
          bundledFilePath: builtin,
          activeFilePath: tsPaths.activeFilePath,
          watch: false,
        });
        const tsSync = new ESCodeBuiltinRemoteSynchronizer({
          source: tsSource,
          controlFilePath: tsPaths.controlFilePath,
          resolveEndpointKey: () => stubTs.origin,
          fetchRelease: (endpointOrigin, signal) =>
            downloadESCodeBuiltinRelease({
              endpointOrigin,
              appVersion: APP_VERSION,
              platform,
              signal,
              request: (url, init) => fetch(url, init),
            }),
        });
        expect(await tsSync.refresh()).toBe("missing");
        tsSync.dispose();
        tsSource.dispose();

        // 请求形状：同一路径与 query（桩的 origin 不同，只比较 path+query）。
        expect(stubA.urls[0]).toBe(stubTs.urls[0]);
        expect(stubA.urls[0]).toBe(
          `/api/v1/client/configs?app_version=${APP_VERSION}&platform=${platform}`,
        );

        // 控制文件：字段一致，时间字段只校验语义（成功收口 = 无租约、一小时后才可再刷）。
        const normalize = (control: Record<string, any>, origin: string) => {
          expect(control.nextEligibleAt).toBeGreaterThan(Date.now() + 3_000_000);
          return {
            ...control,
            endpointKey: control.endpointKey === origin ? "<origin>" : "?",
            nextEligibleAt: "<future>",
          };
        };
        const rustControlFile = JSON.parse(await readFile(rustControl, "utf8"));
        const tsControlFile = JSON.parse(await readFile(tsPaths.controlFilePath, "utf8"));
        expect(normalize(rustControlFile, stubA.origin)).toEqual(
          normalize(tsControlFile, stubTs.origin),
        );
        expect(rustControlFile).toMatchObject({ schemaVersion: 1, leaseUntil: 0, failureCount: 0 });
        expect(rustControlFile.leaseId).toBeUndefined();

        // Endpoint 切换：给 B 预置一份更新的 Active，设置里指向 B；Host 无需重启即切到 B 的缓存。
        const originB = "http://127.0.0.1:1"; // 只作缓存隔离键，不会被请求（下次刷新在 60 s 后）
        const seeded = JSON.parse(bundledText) as Release;
        seeded.revision = bundled.revision + 5;
        seeded.config.providerConfigRules.templateRules.shift();
        const activeB = pathsOf(rustRoot, originB).activeFilePath;
        await mkdir(dirname(activeB), { recursive: true });
        await writeFile(activeB, JSON.stringify(seeded));
        const before = await services.providerSettingsService.getView();
        const settingsFile = join(home, ".escode", "v2", "setting.json");
        await mkdir(dirname(settingsFile), { recursive: true });
        await writeFile(settingsFile, JSON.stringify({ escodeEndpointOrigin: originB }));

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
        try {
          const expected = await runtime.providerSettings.getView();
          const strip = (view: { revision: number }) => ({ ...view, revision: "<n>" });
          let actual = before;
          await waitFor(async () => {
            actual = await services.providerSettingsService.getView();
            return actual.revision > before.revision;
          }, "the view to follow the endpoint override");
          expect(strip(actual)).toEqual(strip(JSON.parse(JSON.stringify(expected))));
          expect(strip(actual)).not.toEqual(strip(before));
        } finally {
          runtime.dispose();
        }
      } finally {
        await host.stop();
        stubA.server.close();
        stubTs.server.close();
        await rm(base, { recursive: true, force: true });
      }
    },
    120_000,
  );
});
