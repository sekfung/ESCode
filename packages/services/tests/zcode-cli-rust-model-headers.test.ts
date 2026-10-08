import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { z } from "zod";
import { fixture, event, end } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-model-request-headers.md：同一夹具与环境下，两侧模型请求的客户端头逐字一致，
// 归因头的键集合、会话类型与 session / trace 归属一致。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const any = z.any();
const STATIC = [
  "http-referer",
  "x-zcode-app-version",
  "x-title",
  "x-release-channel",
  "x-client-language",
  "x-client-timezone",
  "x-zcode-agent",
  "x-platform",
  "x-os-category",
  "x-os-version",
];
const ATTRIBUTION = ["x-request-id", "x-zcode-session-type", "x-zcode-trace-id", "x-session-id", "x-query-id"];
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

async function observe(kind: "node" | "rust") {
  const root = await mkdtemp(join(tmpdir(), `zcode-model-headers-${kind}-`));
  const respond = (_request: any, response: any) => {
    response.writeHead(200, { "content-type": "text/event-stream" });
    event(response, { content: "ok" });
    end(response, "stop");
  };
  const env = { ZCODE_APP_VERSION: "9.8.7" };
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          respond,
          mode: "yolo",
          env,
        })
      : await fixture({ root, registry: true, respond, mode: "yolo", env });
  try {
    await configureRegistry(f);
    const h = f.start();
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "hi" }));
    await h.completed(id);
    const read: any = await h.client.request("session/read", { sessionId: id }, any);
    await h.close();
    const headers = (f as any).requestHeaders.at(-1) as Record<string, string>;
    return {
      static: Object.fromEntries(STATIC.map((key) => [key, headers[key] ?? null])),
      userAgentPrefix: String(headers["user-agent"]).split(" ")[0],
      attributionKeys: ATTRIBUTION.filter((key) => key in headers),
      sessionType: headers["x-zcode-session-type"],
      sessionIdIsStrippedSessionId: headers["x-session-id"] === id.replace(/^sess_/, ""),
      traceIsSessionTrace: headers["x-zcode-trace-id"] === read.session?.traceId,
      requestIdIsUuid: UUID.test(headers["x-request-id"] ?? ""),
      queryIdIsUuid: UUID.test(headers["x-query-id"] ?? ""),
    };
  } finally {
    await f.close();
    await rm(root, { recursive: true, force: true });
  }
}

test("model requests carry the same client and attribution headers on both runtimes", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  // 自检 Node 基准：环境注入的版本与 electron 来源、主轮会话类型、归因与会话一致。
  assert.equal(node.static["x-zcode-app-version"], "9.8.7");
  assert.equal(node.static["x-title"], "Z Code@electron");
  assert.equal(node.userAgentPrefix, "ZCode/9.8.7");
  assert.equal(node.sessionType, "main");
  assert.deepEqual(node.attributionKeys, ATTRIBUTION);
  assert.equal(node.sessionIdIsStrippedSessionId, true);
  assert.equal(node.traceIsSessionTrace, true);
  assert.deepEqual(rust, node);
});
