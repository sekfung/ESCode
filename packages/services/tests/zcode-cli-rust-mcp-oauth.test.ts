import assert from "node:assert/strict";
import test from "node:test";
import { createHash } from "node:crypto";
import { createServer } from "node:http";
import { once } from "node:events";
import { mkdir, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { fixture } from "./zcode-cli-rust-fixture.js";
import { zcodeMcpListResultSchema } from "@zcode/shared";
import { tools } from "./zcode-cli-rust-mcp-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-mcp-oauth.md「验收（第 2 层）」：本地授权服务器 + 需要 Bearer 的 MCP server 上，
// Node 与 Rust 的 DCR/authorize/token 请求、mcp/list 状态序列一致；任一方授权后另一方直接连接；refresh 路径一致。
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
type Kind = "node" | "rust";

async function oauthServer() {
  const log: any[] = [];
  const valid = new Set<string>();
  const challenges = new Map<string, string>();
  let issued = 0;
  let rejectRefresh = false;
  let base = "";
  const json = (res: any, status: number, body: unknown, headers: Record<string, string> = {}) =>
    res
      .writeHead(status, { "Content-Type": "application/json", ...headers })
      .end(JSON.stringify(body));
  const server = createServer(async (req, res) => {
    const url = new URL(req.url ?? "/", base);
    req.setEncoding("utf8");
    let body = "";
    for await (const part of req) body += part;
    if (url.pathname.startsWith("/.well-known/oauth-protected-resource")) {
      log.push({ kind: "resource-metadata", path: url.pathname });
      return json(res, 200, { resource: `${base}/mcp`, authorization_servers: [base] });
    }
    if (url.pathname.startsWith("/.well-known/")) {
      log.push({ kind: "as-metadata", path: url.pathname });
      if (url.pathname !== "/.well-known/oauth-authorization-server") return json(res, 404, {});
      return json(res, 200, {
        issuer: base,
        authorization_endpoint: `${base}/authorize`,
        token_endpoint: `${base}/token`,
        registration_endpoint: `${base}/register`,
        response_types_supported: ["code"],
        grant_types_supported: ["authorization_code", "refresh_token"],
        code_challenge_methods_supported: ["S256"],
        token_endpoint_auth_methods_supported: ["client_secret_basic", "none"],
      });
    }
    if (url.pathname === "/register") {
      const metadata = JSON.parse(body);
      log.push({ kind: "register", metadata });
      return json(res, 201, { ...metadata, client_id: `client-${log.length}` });
    }
    if (url.pathname === "/authorize") {
      const params = Object.fromEntries(url.searchParams);
      log.push({ kind: "authorize", params });
      const code = `code-${log.length}`;
      challenges.set(code, params.code_challenge!);
      const target = new URL(params.redirect_uri!);
      target.searchParams.set("code", code);
      target.searchParams.set("state", params.state!);
      res.writeHead(302, { Location: target.toString() }).end();
      return;
    }
    if (url.pathname === "/token") {
      const params = Object.fromEntries(new URLSearchParams(body));
      log.push({ kind: "token", params, auth: req.headers.authorization ?? null });
      if (params.grant_type === "client_credentials") {
        if (
          req.headers.authorization !==
          `Basic ${Buffer.from("cc-client:cc-secret").toString("base64")}`
        )
          return json(res, 401, { error: "invalid_client" });
      } else if (params.grant_type === "authorization_code") {
        const expected = challenges.get(params.code!);
        const actual = createHash("sha256").update(params.code_verifier!).digest("base64url");
        if (expected !== actual) return json(res, 400, { error: "invalid_grant" });
      } else if (rejectRefresh || params.refresh_token !== `rt-${issued}`) {
        return json(res, 400, { error: "invalid_grant" });
      }
      issued += 1;
      valid.add(`at-${issued}`);
      return json(res, 200, {
        access_token: `at-${issued}`,
        token_type: "Bearer",
        expires_in: 3600,
        refresh_token: `rt-${issued}`,
      });
    }
    // MCP endpoint（streamable HTTP）。
    if (req.method === "DELETE") return res.writeHead(204).end();
    if (req.method === "GET") return res.writeHead(405).end();
    const token = req.headers.authorization?.replace(/^Bearer /, "");
    const m = JSON.parse(body);
    log.push({ kind: "mcp", method: m.method, token: token ?? null });
    if (!token || !valid.has(token)) {
      return json(
        res,
        401,
        { error: "invalid_token" },
        {
          "WWW-Authenticate": `Bearer resource_metadata="${base}/.well-known/oauth-protected-resource/mcp"`,
        },
      );
    }
    if (m.id === undefined) return res.writeHead(202).end();
    const result =
      m.method === "initialize"
        ? {
            protocolVersion: "2025-11-25",
            serverInfo: { name: "fixture", version: "1" },
            capabilities: { tools: {} },
          }
        : { tools };
    return json(res, 200, { jsonrpc: "2.0", id: m.id, result }, { "Mcp-Session-Id": "fixture" });
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("No fixture port");
  base = `http://127.0.0.1:${address.port}`;
  return {
    base,
    log,
    // 协议协商默认值另有差分覆盖；此处固定 legacy，只比较 OAuth 行为。
    config: {
      name: "secure",
      type: "http" as const,
      url: `${base}/mcp`,
      headers: [],
      protocolVersion: "legacy" as const,
      timeoutMs: 5000,
    },
    revokeAll: () => valid.clear(),
    rejectRefresh: () => (rejectRefresh = true),
    async close() {
      server.closeAllConnections();
      await new Promise<void>((r) => server.close(() => r()));
    },
  };
}

/** 端口、随机 state/verifier/client 等每次不同的值归一化，其余逐字段比较。 */
function normalize(value: any, base: string): any {
  return JSON.parse(
    JSON.stringify(value)
      .replaceAll(base, "<as>")
      .replace(/127\.0\.0\.1:\d+/g, "127.0.0.1:<port>")
      .replace(/"(state|code_challenge|code_verifier)":"[^"]+"/g, '"$1":"<random>"')
      .replace(/"(startedAt|updatedAt)":"[^"]+"/g, '"$1":"<time>"'),
  );
}

async function runtime(kind: Kind, root: string) {
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
        })
      : await fixture({ root, registry: true });
  await configureRegistry(f, false);
  return f;
}

/** Node 协议要求 workspaceKey；Rust 同样接受。 */
function listMcp(h: any, workspacePath: string, mcpServers: unknown[], mode = "connect") {
  return h.client.request(
    "mcp/list",
    { workspace: { workspacePath, workspaceKey: workspacePath }, mcpServers, mode },
    zcodeMcpListResultSchema,
  );
}

async function waitStatus(h: any, cwd: string, config: any, done: (s: any) => boolean) {
  const name = config.name;
  for (let i = 0; i < 200; i += 1) {
    const status = (await listMcp(h, cwd, [config], "status")).statuses[name];
    if (status && done(status)) return status;
    await delay(50);
  }
  throw new Error(`MCP ${name} status did not settle`);
}

/** 首次授权：connect 返回待授权状态 → 浏览器完成授权 → 后台重连为 connected。 */
async function authorize(kind: Kind, root: string, as: Awaited<ReturnType<typeof oauthServer>>) {
  const f = await runtime(kind, root);
  try {
    const h = f.start();
    const pending = (await listMcp(h, f.cwd, [as.config])).statuses.secure;
    const url = pending?.authorization?.authorizationUrl;
    assert.ok(url, JSON.stringify(pending));
    const callback = await fetch(url);
    const page = { status: callback.status, text: await callback.text() };
    const settled = await waitStatus(h, f.cwd, as.config, (s) => s.status !== "connecting");
    await h.close();
    const parsed = new URL(url);
    pending.authorization.authorizationUrl = {
      endpoint: `${parsed.origin}${parsed.pathname}`,
      params: Object.fromEntries(parsed.searchParams),
    };
    return { pending, page, settled: { status: settled.status, toolCount: settled.toolCount } };
  } finally {
    await f.close();
  }
}

/** 复用已有凭据：不应再出现 authorize。 */
async function reuse(kind: Kind, root: string, as: Awaited<ReturnType<typeof oauthServer>>) {
  const f = await runtime(kind, root);
  try {
    const h = f.start();
    const status = (await listMcp(h, f.cwd, [as.config])).statuses.secure;
    await h.close();
    return {
      status: status?.status,
      toolCount: status?.toolCount,
      authorization: status?.authorization?.type,
    };
  } finally {
    await f.close();
  }
}

async function scenario(first: Kind, second: Kind) {
  const root = await mkdtemp(join(tmpdir(), "zcode-mcp-oauth-"));
  // Node 的授权租约锁 mkdir 不带 recursive，依赖凭据目录已存在（真实安装总是存在）。
  await mkdir(join(root, ".zcode", "v2"), { recursive: true });
  const as = await oauthServer();
  try {
    const authorized = await authorize(first, root, as);
    const authorizeLog = as.log.splice(0);
    const reused = await reuse(second, root, as);
    const reuseLog = as.log.splice(0);
    // access token 被服务器作废：401 → 锁内 refresh → 以新 token 重试。
    as.revokeAll();
    const refreshed = await reuse(second, root, as);
    const refreshLog = as.log.splice(0);
    // refresh 被拒（invalid_grant）：清 tokens 保留 client，重新进入交互授权。
    as.revokeAll();
    as.rejectRefresh();
    const rejected = await reuse(second, root, as);
    const rejectedLog = as.log.splice(0);
    return normalize(
      { authorized, authorizeLog, reused, reuseLog, refreshed, refreshLog, rejected, rejectedLog },
      as.base,
    );
  } finally {
    await as.close();
    await rm(root, { recursive: true, force: true });
  }
}

test("MCP OAuth authorization, cross-runtime credential reuse and refresh match Node", async () => {
  const node = await scenario("node", "rust");
  const rust = await scenario("rust", "node");
  assert.equal(node.authorized.pending.status, "connecting");
  assert.equal(node.authorized.settled.status, "connected");
  assert.equal(node.reused.status, "connected");
  assert.equal(node.refreshed.status, "connected");
  assert.ok(
    node.refreshLog.some((e: any) => e.kind === "token" && e.params.grant_type === "refresh_token"),
  );
  assert.deepEqual(rust.authorized, node.authorized);
  assert.deepEqual(rust.authorizeLog, node.authorizeLog);
  assert.deepEqual(rust.reused, node.reused);
  assert.deepEqual(rust.refreshed, node.refreshed);
  // 第二段由对方 runtime 执行，请求序列应与「同一对 runtime 反过来」一致。
  assert.deepEqual(rust.reuseLog, node.reuseLog);
  assert.deepEqual(rust.refreshLog, node.refreshLog);
  assert.equal(node.rejected.authorization, "oauth_authorization_code");
  assert.deepEqual(rust.rejected, node.rejected);
  assert.deepEqual(rust.rejectedLog, node.rejectedLog);
});

/** client_credentials：无交互、token 只在内存中；首个请求 401 后取 token 并重试。 */
async function credentials(kind: Kind) {
  const root = await mkdtemp(join(tmpdir(), "zcode-mcp-cc-"));
  await mkdir(join(root, ".zcode", "v2"), { recursive: true });
  const as = await oauthServer();
  try {
    const f = await runtime(kind, root);
    try {
      const h = f.start();
      const config = {
        ...as.config,
        name: "machine",
        oauth: {
          type: "client_credentials" as const,
          clientId: "cc-client",
          clientSecret: "cc-secret",
          scope: "mcp.read",
        },
      };
      const listed = await listMcp(h, f.cwd, [config]);
      const status = listed.statuses.machine;
      await h.close();
      assert.ok(!JSON.stringify(listed).includes("cc-secret"));
      assert.ok(!h.stderr.includes("cc-secret"));
      return normalize(
        { status: status?.status, toolCount: status?.toolCount, log: as.log.splice(0) },
        as.base,
      );
    } finally {
      await f.close();
    }
  } finally {
    await as.close();
    await rm(root, { recursive: true, force: true });
  }
}

test("MCP OAuth client_credentials token acquisition matches Node", async () => {
  const node = await credentials("node");
  const rust = await credentials("rust");
  assert.equal(node.status, "connected");
  assert.ok(
    node.log.some((e: any) => e.kind === "token" && e.params.grant_type === "client_credentials"),
  );
  assert.deepEqual(rust, node);
});
