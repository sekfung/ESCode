// Phase 3 golden freeze：rustHostClientScenes.test.ts 的 TS `createClientScenesService` 观察录制。
// 一次性脚本；删除 TS 实现前用于重录。`--check` 校验仓内 golden 与当前 TS 观察一致。
//
// 归一化：HTTP 桩 origin 写成 `$STUB_ORIGIN`；来源头里的运行时值使用占位符，测试运行时按本机
// 值替换；`x-request-id` 只校验 UUID 形态后从冻结观察中移除（TS/Rust 各自生成，不参与相等比较）。
import { deepEqual } from "node:assert/strict";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { createServer, type IncomingHttpHeaders } from "node:http";
import type { AddressInfo } from "node:net";
import { join } from "node:path";
import { createClientScenesService, createNodeApiClient } from "../packages/services/src/node.ts";

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
  close: () => Promise<void>;
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

/** 只比较与来源头相关的头；`x-device-mid` 取决于本机状态文件，不参与。 */
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

/** 来源头里的运行时值按语义替换为占位符；测试运行时再换成同机值。 */
const HEADER_PLACEHOLDERS: Record<string, string> = {
  "user-agent": "ESCode/$APP_VERSION",
  "http-referer": "$STUB_ORIGIN",
  "x-zcode-app-version": "$APP_VERSION",
  "x-platform": "$PLATFORM",
  "x-release-channel": "$RELEASE_CHANNEL",
  "x-client-language": "$CLIENT_LANGUAGE",
  "x-client-timezone": "$CLIENT_TIMEZONE",
  "x-os-category": "$OS_CATEGORY",
  "x-os-version": "$OS_VERSION",
};

function freezeHeaders(headers: IncomingHttpHeaders): Record<string, string> {
  const frozen = relevant(headers);
  const requestId = frozen["x-request-id"];
  if (!requestId || !UUID.test(requestId)) {
    throw new Error(`x-request-id 不是 UUID: ${String(requestId)}`);
  }
  delete frozen["x-request-id"];
  return Object.fromEntries(
    Object.entries(frozen).map(([name, value]) => [name, HEADER_PLACEHOLDERS[name] ?? value]),
  );
}

const stub = await startStub();
const tsService = createClientScenesService({
  apiClient: createNodeApiClient({ resolveESCodeEndpointOrigin: () => stub.origin }),
});

const modes: Array<{ mode: Mode; result: unknown; url: string; headers: Record<string, string> }> =
  [];
try {
  for (const mode of ["ok", "business-error", "http-error", "text-error", "bad-json"] as const) {
    stub.setMode(mode);
    stub.seen.length = 0;
    const result = await outcome(() => tsService.list());
    if (stub.seen.length !== 1) throw new Error(`${mode} 请求数 ${stub.seen.length}`);
    const seen = stub.seen[0]!;
    modes.push({
      mode,
      // JSON 解析文案是 V8 各自措辞，测试只比较类别与状态码，冻结时折叠掉。
      result:
        mode === "bad-json" && result.ok === false
          ? { ...result, message: "<syntax-error>" }
          : result,
      url: seen.url,
      headers: freezeHeaders(seen.headers),
    });
  }
} finally {
  await stub.close();
}

const golden = {
  _readme:
    "rustHostClientScenes.test.ts 的 TS createClientScenesService 观察冻结（Phase 3 golden freeze）。HTTP 桩 origin 归一化为 $STUB_ORIGIN；来源头运行时值使用占位符，测试运行时替换；x-request-id 仅校验 UUID。",
  modes,
};
const goldenDir = "packages/desktop/test/fixtures/golden";
const goldenPath = join(goldenDir, "rustHostClientScenes.oracle.json");
const json = `${JSON.stringify(golden, null, 2)}\n`;
if (process.argv.includes("--check")) {
  deepEqual(
    JSON.parse(await readFile(goldenPath, "utf8")),
    JSON.parse(json),
    "rustHostClientScenes golden 与当前 TS 观察漂移",
  );
  console.log("rustHostClientScenes golden: OK");
} else {
  await mkdir(goldenDir, { recursive: true });
  await writeFile(goldenPath, json);
  console.log("wrote", goldenPath);
}
