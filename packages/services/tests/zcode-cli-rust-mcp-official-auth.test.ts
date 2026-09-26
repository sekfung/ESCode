import assert from "node:assert/strict";
import test from "node:test";
import { createServer } from "node:http";
import { once } from "node:events";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { zcodeMcpListResultSchema } from "@zcode/shared";
import { fixture } from "./zcode-cli-rust-fixture.js";
import { tools } from "./zcode-cli-rust-mcp-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-mcp-official-auth.md 第 2 期：插件声明 zcode_official 的 http MCP，经 Host 反向请求取身份头。
// Node 与 Rust 在同一 loopback fixture（dev trusted origin）上比较 Host 请求、服务端收到的头与最终状态。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
type Kind = "node" | "rust";
type Mode = "ok" | "rejected" | "untrusted";
const IDENTITY = {
  Authorization: "Bearer identity-jwt",
  "X-Bigmodel-Authorization": "Bearer plan-jwt",
  "Bigmodel-Target-Type": "PERSONAL",
};
const SEEN = [
  "authorization",
  "x-bigmodel-authorization",
  "bigmodel-target-type",
  "x-custom",
  "x-request-id",
];

async function officialServer(mode: Mode) {
  const log: any[] = [];
  const server = createServer(async (req, res) => {
    req.setEncoding("utf8");
    let body = "";
    for await (const part of req) body += part;
    const m = body ? JSON.parse(body) : {};
    log.push({
      http: req.method,
      method: m.method ?? null,
      headers: Object.fromEntries(SEEN.map((name) => [name, req.headers[name] ?? null])),
    });
    if (req.method === "GET") return res.writeHead(405).end();
    if (req.method === "DELETE") return res.writeHead(204).end();
    const headers = { "Content-Type": "application/json", "X-Request-Id": `srv-${log.length}` };
    if (mode === "rejected")
      return res.writeHead(401, headers).end(JSON.stringify({ error: "denied" }));
    if (m.id === undefined) return res.writeHead(202).end();
    if (m.method === "server/discover")
      return res
        .writeHead(200, headers)
        .end(
          JSON.stringify({ jsonrpc: "2.0", id: m.id, error: { code: -32601, message: "legacy" } }),
        );
    const result =
      m.method === "initialize"
        ? {
            protocolVersion: "2025-11-25",
            serverInfo: { name: "official", version: "1" },
            capabilities: { tools: {} },
          }
        : { tools };
    res
      .writeHead(200, { ...headers, "Mcp-Session-Id": "official" })
      .end(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }));
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("No fixture port");
  const origin = `http://127.0.0.1:${address.port}`;
  return {
    origin,
    log,
    async close() {
      server.closeAllConnections();
      await new Promise<void>((r) => server.close(() => r()));
    },
  };
}

async function observe(kind: Kind, mode: Mode) {
  const root = await mkdtemp(join(tmpdir(), "zcode-mcp-official-"));
  const official = await officialServer(mode);
  const env: Record<string, string> =
    mode === "untrusted" ? {} : { ZCODE_OFFICIAL_MCP_DEV_TRUSTED_ORIGINS: official.origin };
  try {
    const f =
      kind === "node"
        ? await fixture({
            root,
            command: process.execPath,
            args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
            registry: true,
            env,
          })
        : await fixture({ root, registry: true, env });
    try {
      await configureRegistry(f, false);
      const plugin = join(root, "official-plugin");
      await mkdir(join(plugin, ".zcode-plugin"), { recursive: true });
      await writeFile(
        join(plugin, ".zcode-plugin/plugin.json"),
        JSON.stringify({ name: "official-fixture" }),
      );
      await writeFile(
        join(plugin, ".mcp.json"),
        JSON.stringify({
          mcpServers: {
            search: {
              type: "http",
              url: `${official.origin}/mcp`,
              // TS 插件解析不读 protocolVersion（白名单字段），两侧都应走默认协商。
              protocolVersion: "legacy",

              headers: { "X-Custom": "static" },
              auth: { type: "zcode_official", provider: "jwt_token" },
            },
          },
        }),
      );
      await mkdir(join(f.cwd, ".zcode"), { recursive: true });
      await writeFile(
        join(f.cwd, ".zcode/config.json"),
        JSON.stringify({ plugins: { dirs: [plugin] } }),
      );
      const h = f.start();
      h.hostHandlers["interaction/requestOfficialMcpAuthHeaders"] = () => ({
        ok: true,
        headers: IDENTITY,
      });
      const listed = await h.client.request(
        "mcp/list",
        { workspace: { workspacePath: f.cwd, workspaceKey: f.cwd }, mode: "connect" },
        zcodeMcpListResultSchema,
      );
      await h.close();
      const [name, status] =
        Object.entries(listed.statuses).find(([key]) => key.includes("search")) ?? [];
      const text = JSON.stringify(listed) + h.stderr;
      assert.ok(!text.includes("identity-jwt") && !text.includes("plan-jwt"), "identity leaked");
      const hostRequests = h.hostRequests
        .filter((r: any) => r.method === "interaction/requestOfficialMcpAuthHeaders")
        .map((r: any) => ({
          ...r.params,
          requestId: "<id>",
          workspace: { ...r.params.workspace, workspaceKey: "<cwd>", workspacePath: "<cwd>" },
        }));
      return {
        name,
        status: status && {
          status: status.status,
          failureKind: status.failureKind ?? null,
          serverRequestId: status.serverRequestId ?? null,
          toolCount: status.toolCount,
        },
        hostRequests: JSON.parse(
          JSON.stringify(hostRequests).replaceAll(official.origin, "<origin>"),
        ),
        server: official.log,
      };
    } finally {
      await f.close();
    }
  } finally {
    await official.close();
    await rm(root, { recursive: true, force: true });
  }
}

for (const mode of ["ok", "rejected", "untrusted"] as const)
  test(`official MCP (${mode}) identity headers, retries and status match Node`, async () => {
    const node = await observe("node", mode);
    const rust = await observe("rust", mode);
    if (mode === "ok") assert.equal(node.status?.status, "connected");
    if (mode === "untrusted") assert.deepEqual(node.server, []);
    assert.deepEqual(rust, node);
  });

/** 第 3 期：官方 stdio MCP 在每条出站请求与通知的 `_meta` 上收到身份载荷（失败时为枚举 reason）。 */
async function observeStdio(kind: Kind, reply: unknown) {
  const root = await mkdtemp(join(tmpdir(), "zcode-mcp-official-stdio-"));
  try {
    const f =
      kind === "node"
        ? await fixture({
            root,
            command: process.execPath,
            args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
            registry: true,
          })
        : await fixture({ root, registry: true });
    try {
      await configureRegistry(f, false);
      const plugin = join(root, "official-stdio");
      const record = join(root, "meta.jsonl");
      await mkdir(join(plugin, ".zcode-plugin"), { recursive: true });
      await writeFile(
        join(plugin, ".zcode-plugin/plugin.json"),
        JSON.stringify({ name: "official-stdio" }),
      );
      await writeFile(
        join(plugin, "server.mjs"),
        `
import { createInterface } from "node:readline";
import { appendFileSync } from "node:fs";
const reply = (m, result) => process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }) + "\\n");
createInterface({ input: process.stdin }).on("line", (line) => {
  const m = JSON.parse(line);
  appendFileSync(${JSON.stringify(record)}, JSON.stringify({ method: m.method ?? null, meta: m.params?._meta?.["com.zcode/official-mcp-auth"] ?? null }) + "\\n");
  if (m.method === "server/discover") process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: m.id, error: { code: -32601, message: "legacy" } }) + "\\n");
  else if (m.method === "initialize") reply(m, { protocolVersion: "2025-11-25", serverInfo: { name: "official-stdio", version: "1" }, capabilities: { tools: {} } });
  else if (m.method === "tools/list") reply(m, { tools: ${JSON.stringify(tools)} });
});
`,
      );
      await writeFile(
        join(plugin, ".mcp.json"),
        JSON.stringify({
          mcpServers: {
            local: {
              command: process.execPath,
              args: ["${CLAUDE_PLUGIN_ROOT}/server.mjs"],
              auth: { type: "zcode_official", provider: "jwt_token" },
            },
          },
        }),
      );
      await mkdir(join(f.cwd, ".zcode"), { recursive: true });
      await writeFile(
        join(f.cwd, ".zcode/config.json"),
        JSON.stringify({ plugins: { dirs: [plugin] } }),
      );
      const h = f.start();
      h.hostHandlers["interaction/requestOfficialMcpAuthHeaders"] = () => reply;
      const listed = await h.client.request(
        "mcp/list",
        { workspace: { workspacePath: f.cwd, workspaceKey: f.cwd }, mode: "connect" },
        zcodeMcpListResultSchema,
      );
      await h.close();
      const status = Object.entries(listed.statuses).find(([key]) => key.includes("local"))?.[1];
      const { readFile } = await import("node:fs/promises");
      const lines = (await readFile(record, "utf8"))
        .trim()
        .split("\n")
        .map((l) => JSON.parse(l));
      const hostRequests = h.hostRequests
        .filter((r: any) => r.method === "interaction/requestOfficialMcpAuthHeaders")
        .map((r: any) => ({ ...r.params, requestId: "<id>", workspace: "<workspace>" }));
      return { status: status?.status, messages: lines, hostRequests };
    } finally {
      await f.close();
    }
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}

for (const [label, reply] of [
  ["ok", { ok: true, headers: IDENTITY }],
  ["plan required", { ok: false, reason: "official_auth_plan_required" }],
] as const)
  test(`official stdio MCP (${label}) _meta identity payload matches Node`, async () => {
    const node = await observeStdio("node", reply);
    const rust = await observeStdio("rust", reply);
    assert.equal(node.status, "connected");
    assert.ok(node.messages.some((m: any) => m.meta !== null));
    assert.deepEqual(rust, node);
  });
