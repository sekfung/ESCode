import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir, version as osVersion } from "node:os";
import { dirname, join } from "node:path";
import { ESCODE_DYNAMIC_WORKFLOW_MODE_ENV, ESCODE_ENV, ESCODE_VERSION } from "@escode/shared";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

// Phase 3 golden freeze：TS `createClientConfigService.getDynamicWorkflowClientConfig` 的观察已冻结
// （packages/desktop/test/fixtures/golden/rustHostDynamicWorkflow.oracle.json）。测试不再实例化 TS
// 服务，删除 TS 实现后仍能独立重验 Rust Host 的 `client-config` 动态工作流读面。
const goldenFile = new URL(
  "./fixtures/golden/rustHostDynamicWorkflow.oracle.json",
  import.meta.url,
);

// docs/specs/rust-u2-client-config.md §5 S3：Rust Host 对同一本地 HTTP 桩求值——远端取值 ×
// 用户选择 × 请求失败 × 环境覆盖，返回值逐项等于冻结观察；缓存与短路行为一致。

interface Stub {
  origin: string;
  requests: () => number;
  reply: (status: number, body: unknown) => void;
  server: Server;
}

interface DynamicWorkflowGolden {
  folds: Array<{ label: string; config: unknown }>;
  shortCircuit: Array<{ choice: string | null; config: unknown }>;
}

async function loadGolden(base: string): Promise<DynamicWorkflowGolden> {
  const raw = (await readFile(goldenFile, "utf8")).split("$FIXTURE_ROOT").join(base);
  return JSON.parse(raw) as DynamicWorkflowGolden;
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

const snapshot = (value: unknown) => JSON.parse(JSON.stringify(value));

describe("rust host client-config.getDynamicWorkflowClientConfig (S3)", () => {
  it.skipIf(!existsSync(hostBinary))(
    "folds remote offer, user choice and failures exactly like the frozen TS observations",
    async () => {
      const base = await realpath(await mkdtemp(join(tmpdir(), "escode-rust-host-dw-")));
      const stub = await startStub();
      const golden = await loadGolden(base);
      // 显式清空继承的本地覆盖，冻结观察对应「无覆盖」场景。
      const { host, home, config } = await startRustHost(base, stub, {
        [ESCODE_DYNAMIC_WORKFLOW_MODE_ENV]: "",
      });
      try {
        let index = 0;
        const compare = async (label: string) => {
          const expected = golden.folds[index];
          if (!expected) throw new Error(`缺少冻结观察 ${label}`);
          expect(expected.label).toBe(label);
          index += 1;
          const actual = await config.getDynamicWorkflowClientConfig({ forceRefresh: true });
          expect({ label, config: snapshot(actual) }).toEqual({
            label,
            config: expected.config,
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
            await setUserMode(home, choice);
            await compare(`offered ${offered}, user ${String(choice)}`);
          }
        }

        // 请求失败：fail-closed 为 disabled/default（两边都只记 warn）。
        await setUserMode(home, undefined);
        stub.reply(503, "{}");
        const failed = await compare("http 503");
        expect(failed).toMatchObject({ mode: "disabled", source: "default", enabled: false });
        stub.reply(200, "<html>");
        await compare("unparseable body");
        expect(index).toBe(golden.folds.length);

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
      const golden = await loadGolden(base);
      stub.reply(200, envelope({ mode: "disabled" }));
      const { host, home, config } = await startRustHost(base, stub, {
        [ESCODE_DYNAMIC_WORKFLOW_MODE_ENV]: "onDemand",
      });
      try {
        const choices = [undefined, "alwaysOn", "disabled"] as const;
        for (const [index, choice] of choices.entries()) {
          const expected = golden.shortCircuit[index];
          if (!expected) throw new Error(`缺少冻结的本地覆盖观察 ${index}`);
          expect(expected.choice).toBe(choice ?? null);
          await setUserMode(home, choice);
          const actual = await config.getDynamicWorkflowClientConfig({ forceRefresh: true });
          expect({ choice: choice ?? null, config: snapshot(actual) }).toEqual({
            choice: choice ?? null,
            config: expected.config,
          });
          expect(actual).toMatchObject({ source: "override" });
        }
        expect(stub.requests(), "覆盖合法即短路，不发请求").toBe(0);
      } finally {
        await host.stop();
        stub.server.close();
        await rm(base, { recursive: true, force: true });
      }
    },
    120_000,
  );
});
