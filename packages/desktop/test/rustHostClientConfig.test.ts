import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, realpath, rm } from "node:fs/promises";
import { createServer, type IncomingHttpHeaders, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir, version as osVersion } from "node:os";
import { join } from "node:path";
import { ESCODE_ENV, ESCODE_VERSION } from "@escode/shared";
import { createClientConfigService, createNodeApiClient } from "@escode/services/node";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

// docs/specs/rust-u2-client-config.md §5 S2：本地 HTTP 桩作 Endpoint，真实 TS `createClientConfigService` 与 Rust Host 的
// `client-config.getSnapshot` 对同一桩求值——快照、请求次数（缓存/forceRefresh）、请求头与错误逐项相等。

interface Seen {
  url: string;
  headers: IncomingHttpHeaders;
}

type Reply = { status: number; body: string };

const MESSY = {
  code: 0,
  data: {
    configs: {
      pluginStoreOrder: {
        code: {
          categoryOrder: ["b", " a ", "b", "a"],
          pluginOrder: { m: ["x", "x ", " y"] },
          extra: 1,
        },
        work: "broken",
        other: { categoryOrder: ["z"] },
      },
      zsrc: { url: "  https://zsrc.example.com/path  ", extra: true },
    },
  },
};

async function startStub(): Promise<{
  origin: string;
  seen: Seen[];
  reply: (next: Reply) => void;
  server: Server;
}> {
  const seen: Seen[] = [];
  let current: Reply = { status: 200, body: JSON.stringify(MESSY) };
  const server = createServer((request, response) => {
    seen.push({ url: request.url ?? "", headers: request.headers });
    response.statusCode = current.status;
    response.setHeader("content-type", "application/json");
    response.end(current.body);
  });
  await new Promise<void>((done) => server.listen(0, "127.0.0.1", done));
  const { port } = server.address() as AddressInfo;
  return {
    origin: `http://127.0.0.1:${port}`,
    seen,
    reply: (next) => {
      current = next;
    },
    server,
  };
}

async function outcome<T>(run: () => Promise<T>) {
  try {
    return { ok: true as const, value: JSON.parse(JSON.stringify(await run())) };
  } catch (error) {
    const e = error as Error & { status?: number };
    return {
      ok: false as const,
      name: e.name,
      message: e.message,
      ...(e.status === undefined ? {} : { status: e.status }),
    };
  }
}

function relevant(headers: IncomingHttpHeaders): Record<string, string> {
  const result: Record<string, string> = {};
  for (const [name, value] of Object.entries(headers)) {
    if (name === "user-agent" || name === "referer" || name.startsWith("x-")) {
      if (name === "x-device-mid" || name === "x-request-id") continue;
      result[name] = Array.isArray(value) ? value.join(",") : String(value);
    }
  }
  return result;
}

describe("rust host client-config.getSnapshot (S2)", () => {
  it.skipIf(!existsSync(hostBinary))(
    "matches the TS service for snapshots, caching, forceRefresh, errors and option validation",
    async () => {
      const base = await realpath(await mkdtemp(join(tmpdir(), "escode-rust-host-config-")));
      const home = join(base, "home");
      await mkdir(home, { recursive: true });
      const stub = await startStub();
      const language = Intl.DateTimeFormat().resolvedOptions().locale;
      const timezone = Intl.DateTimeFormat().resolvedOptions().timeZone;
      const makeTs = () =>
        createClientConfigService({
          apiClient: createNodeApiClient({ resolveESCodeEndpointOrigin: () => stub.origin }),
          resolveRequestContext: async () => ({
            endpointOrigin: stub.origin,
            appVersion: ESCODE_VERSION,
            platform: `${process.platform}-${process.arch}`,
          }),
        });
      const host = startHost(home);
      try {
        expect((await nextMeaningful(host)).kind).toBe("hostReady");
        host.send({
          kind: "rpcListen",
          bind: "127.0.0.1:0",
          home,
          runtimeEnv: {
            ESCODE_APP_VERSION: ESCODE_VERSION,
            ESCODE_ENV,
            ESCODE_ENDPOINT_ORIGIN: stub.origin,
            ESCODE_CLIENT_LANGUAGE: language,
            ESCODE_CLIENT_TIMEZONE: timezone,
            ESCODE_OS_VERSION: osVersion(),
            ESCODE_BUILTIN_CACHE_ROOT: base,
          },
        });
        const listening = await nextMeaningful(host);
        const rust = (await connectRpc(String(listening.addr), String(listening.secret)))
          .clientConfigService;

        // 每个场景用全新的 TS 服务实例（它自带缓存）；Rust 的缓存在 Host 里，靠 forceRefresh 重置。
        const compare = async (
          label: string,
          options: unknown,
          { sameMessage = true }: { sameMessage?: boolean } = {},
        ) => {
          const ts = makeTs();
          stub.seen.length = 0;
          const expected = await outcome(() => ts.getSnapshot(options as never));
          const tsSeen = stub.seen.length;
          const actual = await outcome(() => rust.getSnapshot(options as never));
          if (sameMessage) expect({ label, result: actual }).toEqual({ label, result: expected });
          else {
            expect(actual, label).toMatchObject({
              ok: false,
              name: (expected as { name: string }).name,
            });
          }
          return { tsSeen, rustSeen: stub.seen.length - tsSeen, seen: [...stub.seen] };
        };

        // 1) 成功：快照逐值相等；两边各发一次相同路径与 query、相同来源头。
        stub.reply({ status: 200, body: JSON.stringify(MESSY) });
        const first = await compare("messy envelope", { forceRefresh: true });
        expect(first).toMatchObject({ tsSeen: 1, rustSeen: 1 });
        expect(first.seen[1]!.url).toBe(first.seen[0]!.url);
        expect(first.seen[0]!.url).toBe(
          `/api/v1/client/configs?app_version=${ESCODE_VERSION}&platform=${process.platform}-${process.arch}`,
        );
        expect(relevant(first.seen[1]!.headers)).toEqual(relevant(first.seen[0]!.headers));
        for (const request of first.seen) {
          expect(request.headers.authorization).toBeUndefined();
          expect(request.headers.cookie).toBeUndefined();
        }

        // 2) 缓存：Rust 在 TTL 内不再请求（TS 实例是新的，所以这里只断言 Rust 一侧 + 值相等）。
        const before = stub.seen.length;
        const cached = await outcome(() => rust.getSnapshot());
        expect(stub.seen.length).toBe(before);
        expect(cached.ok).toBe(true);
        // forceRefresh 绕过缓存，恰好一次请求。
        await rust.getSnapshot({ forceRefresh: true });
        expect(stub.seen.length).toBe(before + 1);

        // 3) 缺省选项 / 未知键。
        await compare("no options", undefined);
        await compare("unknown option keys", { forceRefresh: true, extra: 1 });

        // 4) 非法选项：两边都是 ZodError（文案是各自的措辞，只比类别）。
        for (const bad of [null, "yes", { forceRefresh: "yes" }, { forceRefresh: null }, []]) {
          const ts = makeTs();
          const [expected, actual] = [
            await outcome(() => ts.getSnapshot(bad as never)),
            await outcome(() => rust.getSnapshot(bad as never)),
          ];
          expect({ bad, ok: actual.ok, name: (actual as { name?: string }).name }).toEqual({
            bad,
            ok: expected.ok,
            name: (expected as { name?: string }).name,
          });
        }

        // 5) 错误：HTTP 状态、非法信封、坏 JSON。
        stub.reply({ status: 503, body: "{}" });
        await compare("http 503", { forceRefresh: true });
        stub.reply({ status: 200, body: JSON.stringify({ code: 1 }) });
        await compare("invalid envelope", { forceRefresh: true });
        stub.reply({ status: 200, body: "<html>" });
        await compare("bad json", { forceRefresh: true }, { sameMessage: false });
        // 失败不写缓存、也不清缓存：最近一次成功的快照仍在 TTL 内，不带 forceRefresh 的读取不发请求。
        const beforeRecovery = stub.seen.length;
        const retained = await outcome(() => rust.getSnapshot());
        expect(stub.seen.length).toBe(beforeRecovery);
        expect(retained).toMatchObject({ ok: true });
        expect((retained as { value: { zsrcUrl: unknown } }).value.zsrcUrl).toBe(
          "https://zsrc.example.com/path",
        );
        // 恢复后 forceRefresh 拿到新值，与 TS 一致（`configs: null` → 两个字段都为 null）。
        stub.reply({ status: 200, body: JSON.stringify({ code: 0, data: { configs: null } }) });
        await compare("recovers after failures", { forceRefresh: true });
      } finally {
        await host.stop();
        stub.server.close();
        await rm(base, { recursive: true, force: true });
      }
    },
    120_000,
  );
});
