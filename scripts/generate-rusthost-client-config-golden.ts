// Phase 3 golden freeze：rustHostClientConfig.test.ts 的 TS `createClientConfigService` 观察录制。
// 一次性脚本；删除 TS 实现前用于重录。`--check` 校验仓内 golden 与当前 TS 观察一致。
//
// 归一化：临时根路径写成 `$FIXTURE_ROOT`，HTTP 桩 origin 写成 `$STUB_ORIGIN`；来源头里的运行时值
// （版本/平台/语言/时区/OS）写成占位符，测试运行时按本机值替换。仅在 TS 侧文案不参与比较的
// 场景（JSON 解析错误）折叠成固定占位，避免 Node 版本差异造成 --check 漂移。
import { deepEqual } from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createServer, type IncomingHttpHeaders } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { ESCODE_VERSION } from "../packages/shared/src/index.ts";
import { createClientConfigService, createNodeApiClient } from "../packages/services/src/node.ts";

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

interface Seen {
  url: string;
  headers: IncomingHttpHeaders;
}

type Reply = { status: number; body: string };

async function startStub(): Promise<{
  origin: string;
  seen: Seen[];
  reply: (next: Reply) => void;
  close: () => Promise<void>;
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
    close: () => new Promise((done) => server.close(() => done())),
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

/** 来源头里的运行时值按语义替换为占位符；测试运行时再换成同机值。 */
const HEADER_PLACEHOLDERS: Record<string, string> = {
  "user-agent": "ESCode/$APP_VERSION",
  "x-zcode-app-version": "$APP_VERSION",
  "x-platform": "$PLATFORM",
  "x-release-channel": "$RELEASE_CHANNEL",
  "x-client-language": "$CLIENT_LANGUAGE",
  "x-client-timezone": "$CLIENT_TIMEZONE",
  "x-os-category": "$OS_CATEGORY",
  "x-os-version": "$OS_VERSION",
};

function normalizeHeaders(headers: Record<string, string>): Record<string, string> {
  return Object.fromEntries(
    Object.entries(headers).map(([name, value]) => [name, HEADER_PLACEHOLDERS[name] ?? value]),
  );
}

const root = await mkdtemp(join(tmpdir(), "rusthost-client-config-golden-"));
const stub = await startStub();
const makeTs = () =>
  createClientConfigService({
    apiClient: createNodeApiClient({ resolveESCodeEndpointOrigin: () => stub.origin }),
    resolveRequestContext: async () => ({
      endpointOrigin: stub.origin,
      appVersion: ESCODE_VERSION,
      platform: `${process.platform}-${process.arch}`,
    }),
  });

/** 每个场景用全新 TS 实例（它自带缓存）；记录结果与本次 TS 发出的请求数。 */
async function scenario(label: string, options: unknown) {
  const ts = makeTs();
  stub.seen.length = 0;
  const result = await outcome(() => ts.getSnapshot(options as never));
  return { label, result, seen: [...stub.seen] };
}

const outcomes: Record<string, unknown> = {};
const invalidOptions: unknown[] = [];
let request: { count: number; url: string; headers: Record<string, string> } | undefined;

try {
  stub.reply({ status: 200, body: JSON.stringify(MESSY) });
  const first = await scenario("messy envelope", { forceRefresh: true });
  outcomes[first.label] = first.result;
  if (first.seen.length !== 1) throw new Error(`messy envelope 请求数 ${first.seen.length}`);
  const requestSeen = first.seen[0]!;
  request = {
    count: first.seen.length,
    // query 里的版本/平台随仓库版本与运行机器变化，冻结为占位符，测试运行时替换。
    url: requestSeen.url
      .split(`${process.platform}-${process.arch}`)
      .join("$PLATFORM")
      .split(ESCODE_VERSION)
      .join("$APP_VERSION"),
    headers: normalizeHeaders(relevant(requestSeen.headers)),
  };

  outcomes["no options"] = (await scenario("no options", undefined)).result;
  outcomes["unknown option keys"] = (
    await scenario("unknown option keys", { forceRefresh: true, extra: 1 })
  ).result;

  for (const bad of [null, "yes", { forceRefresh: "yes" }, { forceRefresh: null }, []]) {
    const result = await outcome(() => makeTs().getSnapshot(bad as never));
    // 只保留测试实际比较的类别（ok/name），Zod 文案随版本变化不进入冻结观察。
    invalidOptions.push({
      bad,
      ok: result.ok,
      name: result.ok ? undefined : result.name,
    });
  }

  stub.reply({ status: 503, body: "{}" });
  outcomes["http 503"] = (await scenario("http 503", { forceRefresh: true })).result;
  stub.reply({ status: 200, body: JSON.stringify({ code: 1 }) });
  outcomes["invalid envelope"] = (
    await scenario("invalid envelope", { forceRefresh: true })
  ).result;
  stub.reply({ status: 200, body: "<html>" });
  const badJson = (await scenario("bad json", { forceRefresh: true })).result;
  // 测试只比较 ok/name；V8 的 JSON 解析文案不稳定，折叠掉。
  outcomes["bad json"] = badJson.ok === false ? { ...badJson, message: "<syntax-error>" } : badJson;
  stub.reply({ status: 200, body: JSON.stringify({ code: 0, data: { configs: null } }) });
  outcomes["recovers after failures"] = (
    await scenario("recovers after failures", { forceRefresh: true })
  ).result;
} finally {
  await stub.close();
  await rm(root, { recursive: true, force: true });
}

if (!request) throw new Error("missing messy-envelope request observation");
const golden = {
  _readme:
    "rustHostClientConfig.test.ts 的 TS createClientConfigService 观察冻结（Phase 3 golden freeze）。临时根归一化为 $FIXTURE_ROOT，HTTP 桩 origin 归一化为 $STUB_ORIGIN；来源头运行时值使用占位符，测试运行时替换。",
  outcomes,
  invalidOptions,
  request,
};
const goldenDir = "packages/desktop/test/fixtures/golden";
const goldenPath = join(goldenDir, "rustHostClientConfig.oracle.json");
const json = `${JSON.stringify(golden, null, 2)}\n`;
if (process.argv.includes("--check")) {
  deepEqual(
    JSON.parse(await readFile(goldenPath, "utf8")),
    JSON.parse(json),
    "rustHostClientConfig golden 与当前 TS 观察漂移",
  );
  console.log("rustHostClientConfig golden: OK");
} else {
  await mkdir(goldenDir, { recursive: true });
  await writeFile(goldenPath, json);
  console.log("wrote", goldenPath);
}
