import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, realpath, rm, writeFile } from "node:fs/promises";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir, version as osVersion } from "node:os";
import { dirname, join } from "node:path";
import { ESCODE_DYNAMIC_WORKFLOW_MODE_ENV, ESCODE_ENV, ESCODE_VERSION } from "@escode/shared";
import { createClientConfigService, createNodeApiClient } from "@escode/services/node";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

// docs/specs/rust-u2-client-config.md §5 S3：真实 TS `createClientConfigService.getDynamicWorkflowClientConfig` 与 Rust Host
// 对同一本地 HTTP 桩求值——远端取值 × 用户选择 × 请求失败 × 环境覆盖，返回值逐项相等；缓存与短路行为一致。

interface Stub {
  origin: string;
  requests: () => number;
  reply: (status: number, body: unknown) => void;
  server: Server;
}

async function startStub(): Promise<Stub> {
  let count = 0;
  let current = { status: 200, body: "{}" };
  const server = createServer((_request, response) => {
    count += 1;
    response.statusCode = current.status;
    response.setHeader("content-type", "application/json");
    response.end(current.body);
  });
  await new Promise<void>((done) => server.listen(0, "127.0.0.1", done));
  const { port } = server.address() as AddressInfo;
  return {
    origin: `http://127.0.0.1:${port}`,
    requests: () => count,
    reply: (status, body) => {
      current = { status, body: typeof body === "string" ? body : JSON.stringify(body) };
    },
    server,
  };
}

const envelope = (dynamicWorkflow: unknown) => ({
  code: 0,
  data: { configs: { dynamicWorkflow } },
});

async function startRustHost(base: string, stub: Stub, extraEnv: Record<string, string> = {}) {
  const home = join(base, "home");
  await mkdir(home, { recursive: true });
  const host = startHost(home);
  expect((await nextMeaningful(host)).kind).toBe("hostReady");
  host.send({
    kind: "rpcListen",
    bind: "127.0.0.1:0",
    home,
    runtimeEnv: {
      ESCODE_APP_VERSION: ESCODE_VERSION,
      ESCODE_ENV,
      ESCODE_ENDPOINT_ORIGIN: stub.origin,
      ESCODE_CLIENT_LANGUAGE: Intl.DateTimeFormat().resolvedOptions().locale,
      ESCODE_CLIENT_TIMEZONE: Intl.DateTimeFormat().resolvedOptions().timeZone,
      ESCODE_OS_VERSION: osVersion(),
      ESCODE_BUILTIN_CACHE_ROOT: base,
      ...extraEnv,
    },
  });
  const listening = await nextMeaningful(host);
  const services = await connectRpc(String(listening.addr), String(listening.secret));
  return { host, home, config: services.clientConfigService };
}

async function setUserMode(home: string, mode: unknown) {
  const file = join(home, ".escode", "v2", "setting.json");
  await mkdir(dirname(file), { recursive: true });
  await writeFile(file, JSON.stringify(mode === undefined ? {} : { dynamicWorkflowMode: mode }));
}

function tsService(stub: Stub, userMode: () => Promise<unknown>) {
  return createClientConfigService({
    apiClient: createNodeApiClient({ resolveESCodeEndpointOrigin: () => stub.origin }),
    resolveRequestContext: async () => ({
      endpointOrigin: stub.origin,
      appVersion: ESCODE_VERSION,
      platform: `${process.platform}-${process.arch}`,
    }),
    resolveDynamicWorkflowUserMode: userMode,
  });
}

describe("rust host client-config.getDynamicWorkflowClientConfig (S3)", () => {
  it.skipIf(!existsSync(hostBinary))(
    "folds remote offer, user choice and failures exactly like the TS service",
    async () => {
      const base = await realpath(await mkdtemp(join(tmpdir(), "escode-rust-host-dw-")));
      const stub = await startStub();
      const { host, home, config } = await startRustHost(base, stub);
      try {
        let userMode: unknown;
        const ts = () => tsService(stub, async () => userMode);
        const compare = async (label: string) => {
          const expected = await ts().getDynamicWorkflowClientConfig({ forceRefresh: true });
          const actual = await config.getDynamicWorkflowClientConfig({ forceRefresh: true });
          expect({ label, config: JSON.parse(JSON.stringify(actual)) }).toEqual({
            label,
            config: JSON.parse(JSON.stringify(expected)),
          });
          return actual;
        };

        // 远端取值（含非法值、缺 key、configs 为 null、业务码非 0——原始信封不校验）。
        for (const remote of [
          { mode: "onDemand" },
          { mode: "alwaysOn" },
          { mode: "disabled" },
          { mode: " onDemand " },
          { mode: "bogus" },
          { mode: 1 },
          "x",
          null,
        ]) {
          stub.reply(200, envelope(remote));
          await compare(`remote ${JSON.stringify(remote)}`);
        }
        stub.reply(200, { code: 0, data: { configs: {} } });
        await compare("no dynamicWorkflow key");
        stub.reply(200, { code: 0, data: { configs: null } });
        await compare("configs null");
        stub.reply(200, { code: 1, data: { configs: { dynamicWorkflow: { mode: "alwaysOn" } } } });
        await compare("business code is not validated here");

        // 用户选择：只在功能被提供时生效；非法值按“跟随”处理；选择变化不触发网络。
        for (const offered of ["onDemand", "disabled"]) {
          stub.reply(200, envelope({ mode: offered }));
          for (const choice of ["alwaysOn", "onDemand", "disabled", "bogus", undefined]) {
            userMode = choice;
            await setUserMode(home, choice);
            await compare(`offered ${offered}, user ${String(choice)}`);
          }
        }

        // 请求失败：fail-closed 为 disabled/default（两边都只记 warn）。
        userMode = undefined;
        await setUserMode(home, undefined);
        stub.reply(503, "{}");
        const failed = await compare("http 503");
        expect(failed).toMatchObject({ mode: "disabled", source: "default", enabled: false });
        stub.reply(200, "<html>");
        await compare("unparseable body");

        // 缓存：1 h TTL 内不带 forceRefresh 不再请求；失败没有污染它。
        stub.reply(200, envelope({ mode: "alwaysOn" }));
        await config.getDynamicWorkflowClientConfig({ forceRefresh: true });
        const before = stub.requests();
        const cached = await config.getDynamicWorkflowClientConfig();
        expect(stub.requests()).toBe(before);
        expect(cached).toMatchObject({ mode: "alwaysOn", source: "remote" });
      } finally {
        await host.stop();
        stub.server.close();
        await rm(base, { recursive: true, force: true });
      }
    },
    120_000,
  );

  it.skipIf(!existsSync(hostBinary))(
    "short-circuits on a valid local override without any request",
    async () => {
      const base = await realpath(await mkdtemp(join(tmpdir(), "escode-rust-host-dw-env-")));
      const stub = await startStub();
      stub.reply(200, envelope({ mode: "disabled" }));
      const { host, home, config } = await startRustHost(base, stub, {
        [ESCODE_DYNAMIC_WORKFLOW_MODE_ENV]: "onDemand",
      });
      const saved = process.env[ESCODE_DYNAMIC_WORKFLOW_MODE_ENV];
      process.env[ESCODE_DYNAMIC_WORKFLOW_MODE_ENV] = "onDemand";
      try {
        for (const choice of [undefined, "alwaysOn", "disabled"]) {
          await setUserMode(home, choice);
          const expected = await tsService(stub, async () => choice).getDynamicWorkflowClientConfig(
            {
              forceRefresh: true,
            },
          );
          const actual = await config.getDynamicWorkflowClientConfig({ forceRefresh: true });
          expect({ choice, config: JSON.parse(JSON.stringify(actual)) }).toEqual({
            choice,
            config: JSON.parse(JSON.stringify(expected)),
          });
          expect(actual).toMatchObject({ source: "override" });
        }
        expect(stub.requests(), "覆盖合法即短路，两边都不发请求").toBe(0);
      } finally {
        if (saved === undefined) delete process.env[ESCODE_DYNAMIC_WORKFLOW_MODE_ENV];
        else process.env[ESCODE_DYNAMIC_WORKFLOW_MODE_ENV] = saved;
        await host.stop();
        stub.server.close();
        await rm(base, { recursive: true, force: true });
      }
    },
    120_000,
  );
});
