import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, realpath, rm } from "node:fs/promises";
import { createServer, type IncomingHttpHeaders, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir, version as osVersion } from "node:os";
import { join } from "node:path";
import { ESCODE_ENV, ESCODE_VERSION } from "@escode/shared";
import { createClientScenesService, createNodeApiClient } from "@escode/services/node";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

// docs/specs/rust-u2-client-config.md §5 S1：本地 HTTP 桩作 Endpoint，真实 TS `createClientScenesService` + `createNodeApiClient`
// 与 Rust Host 的 `client-scenes.list` 对同一桩求值——请求路径/头集合与应答/错误逐项相等。

type Mode = "ok" | "http-error" | "text-error" | "bad-json" | "business-error";

interface Seen {
  url: string;
  headers: IncomingHttpHeaders;
}

const SCENES = {
  code: 0,
  msg: "ok",
  data: [
    {
      namespace: "home",
      scene: "draft",
      options: {
        topic: {
          id: "topic",
          type: "topic",
          contents: { en: "Topic", zh: "主题" },
          items: [{ id: "a", type: "t", contents: { en: "A" }, labels: { en: "L" }, img: null }],
        },
      },
      created_at: 1,
    },
  ],
};

async function startStub(): Promise<{
  origin: string;
  seen: Seen[];
  setMode: (mode: Mode) => void;
  server: Server;
}> {
  const seen: Seen[] = [];
  let mode: Mode = "ok";
  const server = createServer((request, response) => {
    seen.push({ url: request.url ?? "", headers: request.headers });
    response.setHeader("content-type", "application/json");
    response.setHeader("x-request-id", "srv-req-1");
    if (mode === "ok") response.end(JSON.stringify(SCENES));
    else if (mode === "http-error") {
      response.statusCode = 500;
      response.end(JSON.stringify({ error: "boom", message: "ignored" }));
    } else if (mode === "text-error") {
      response.statusCode = 502;
      response.setHeader("content-type", "text/plain");
      response.end("  upstream down \n");
    } else if (mode === "bad-json") response.end("<html>");
    else response.end(JSON.stringify({ code: 40101, msg: "not allowed", data: [] }));
  });
  await new Promise<void>((done) => server.listen(0, "127.0.0.1", done));
  const { port } = server.address() as AddressInfo;
  return {
    origin: `http://127.0.0.1:${port}`,
    seen,
    setMode: (next) => {
      mode = next;
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

/** 只比较与来源头相关的头；`x-request-id` 只校验是 UUID，`x-device-mid` 取决于本机状态文件，不参与。 */
function relevant(headers: IncomingHttpHeaders): Record<string, string> {
  const result: Record<string, string> = {};
  for (const [name, value] of Object.entries(headers)) {
    if (
      name === "user-agent" ||
      name === "referer" ||
      name === "http-referer" ||
      name.startsWith("x-")
    ) {
      if (name === "x-device-mid") continue;
      result[name] = Array.isArray(value) ? value.join(",") : String(value);
    }
  }
  return result;
}

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

describe("rust host client-scenes (S1)", () => {
  it.skipIf(!existsSync(hostBinary))(
    "matches the TS service for requests, source headers, responses and errors",
    async () => {
      const base = await realpath(await mkdtemp(join(tmpdir(), "escode-rust-host-scenes-")));
      const home = join(base, "home");
      await mkdir(home, { recursive: true });
      const stub = await startStub();
      const language = Intl.DateTimeFormat().resolvedOptions().locale;
      const timezone = Intl.DateTimeFormat().resolvedOptions().timeZone;
      const tsService = createClientScenesService({
        apiClient: createNodeApiClient({ resolveESCodeEndpointOrigin: () => stub.origin }),
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
        const services = await connectRpc(String(listening.addr), String(listening.secret));

        for (const mode of [
          "ok",
          "business-error",
          "http-error",
          "text-error",
          "bad-json",
        ] as const) {
          stub.setMode(mode);
          stub.seen.length = 0;
          const expected = await outcome(() => tsService.list());
          const actual = await outcome(() => services.clientScenesService.list());
          if (mode === "bad-json") {
            // 登记差异（spec §6 ⑤）：JSON 解析错误的文案是 V8 与 serde 各自的措辞，只比较类别与状态码。
            expect(actual).toMatchObject({ ok: false, name: "ApiError", status: 200 });
            expect(expected).toMatchObject({ ok: false, name: "ApiError", status: 200 });
            expect((actual as { message: string }).message.length).toBeGreaterThan(0);
          } else {
            expect({ mode, result: actual }).toEqual({ mode, result: expected });
          }

          expect(stub.seen).toHaveLength(2);
          const [tsRequest, rustRequest] = stub.seen;
          expect(rustRequest!.url, mode).toBe(tsRequest!.url);
          expect(rustRequest!.url).toBe("/api/v1/client/scenes");
          const [tsHeaders, rustHeaders] = [
            relevant(tsRequest!.headers),
            relevant(rustRequest!.headers),
          ];
          for (const headers of [tsHeaders, rustHeaders]) {
            expect(headers["x-request-id"], mode).toMatch(UUID);
            delete headers["x-request-id"];
          }
          expect({ mode, headers: rustHeaders }).toEqual({ mode, headers: tsHeaders });
          // 无凭据：任何一边都不带 Authorization / Cookie。
          for (const request of stub.seen) {
            expect(request.headers.authorization).toBeUndefined();
            expect(request.headers.cookie).toBeUndefined();
          }
        }
      } finally {
        await host.stop();
        stub.server.close();
        await rm(base, { recursive: true, force: true });
      }
    },
    120_000,
  );
});
