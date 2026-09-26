// Run with node --import tsx. MCP OAuth 纯规则的 TS oracle（docs/specs/rust-mcp-oauth.md 第 2 层）：
// 凭据 key、pair 派生、scope 并集、WWW-Authenticate、discovery URL、resource 选择、client 认证方式、client metadata。
// --check 防漂移。
import { readFile, writeFile } from "node:fs/promises";
import {
  buildDiscoveryUrls,
  checkResourceAllowed,
  computeScopeUnion,
  extractWWWAuthenticateParams,
  resolveClientMetadata,
  resourceUrlFromServerUrl,
  selectClientAuthMethod,
} from "@modelcontextprotocol/client";
import { createCredentialKeyPrefix } from "../apps/zcode-cli/packages/adapters/src/mcp/oauth.ts";
import { deriveCredentialPair } from "../apps/zcode-cli/packages/adapters/src/mcp/oauth-credentials.ts";
import { sanitizeKeyPrefix } from "../apps/zcode-cli/packages/adapters/src/mcp/oauth-lease.ts";

const keyPrefixes = [
  ["linear", "https://mcp.linear.app/sse", {}],
  ["notion", "https://mcp.notion.com/mcp", { clientId: "cid", scope: "read write" }],
  ["名字", "http://127.0.0.1:9/mcp?x=1", { redirectPath: "/cb" }],
].map(([name, url, config]) => {
  const prefix = createCredentialKeyPrefix(name, url, { type: "authorization_code", ...config });
  return { name, url, config, prefix, sanitized: sanitizeKeyPrefix(prefix) };
});

const client = { client_id: "c1", issuer: "https://as.example" };
const tokens = { access_token: "a1", token_type: "Bearer", refresh_token: "r1" };
const tokens2 = { access_token: "a2", token_type: "Bearer" };
const canonical = (extra) =>
  JSON.stringify({
    client_information: client,
    generation: "g1",
    published_by: "tx",
    tokens,
    version: 2,
    ...extra,
  });
const pairInputs = [
  {},
  { legacyClientRaw: JSON.stringify(client) },
  { legacyTokensRaw: JSON.stringify(tokens) },
  { canonicalRaw: canonical() },
  { canonicalRaw: canonical(), legacyClientRaw: JSON.stringify(client) },
  {
    canonicalRaw: canonical(),
    legacyClientRaw: JSON.stringify(client),
    legacyTokensRaw: JSON.stringify(tokens),
  },
  {
    canonicalRaw: canonical(),
    legacyClientRaw: JSON.stringify(client),
    legacyTokensRaw: JSON.stringify(tokens2),
  },
  {
    canonicalRaw: canonical(),
    legacyClientRaw: JSON.stringify({ client_id: "other" }),
    legacyTokensRaw: JSON.stringify(tokens2),
  },
  { canonicalRaw: canonical({ version: 1, generation: undefined }) },
  {
    canonicalRaw: canonical({ version: 1 }),
    legacyTokensRaw: JSON.stringify(tokens2),
  },
  { canonicalRaw: canonical({ version: 3 }), legacyClientRaw: JSON.stringify(client) },
  { canonicalRaw: "not json", legacyTokensRaw: JSON.stringify(tokens) },
  { canonicalRaw: canonical({ expires_at: 5, obtained_at: 1, issuer: "i" }) },
  // 缺 generation 的旧 v2 记录：generation 取原始内容 hash。
  {
    canonicalRaw: JSON.stringify({
      client_information: client,
      published_by: "tx",
      tokens,
      version: 2,
    }),
    legacyClientRaw: JSON.stringify(client),
    legacyTokensRaw: JSON.stringify(tokens),
  },
].map((input) => ({ input, pair: deriveCredentialPair(input) ?? null }));

const scopeUnions = [
  [undefined, undefined],
  ["a b", "b c"],
  ["  a  ", undefined, "c a d"],
  ["", "x"],
].map((scopes) => ({ scopes, union: computeScopeUnion(...scopes) ?? null }));

const wwwAuthenticate = [
  'Bearer resource_metadata="https://mcp.example/.well-known/oauth-protected-resource"',
  'Bearer error="insufficient_scope", scope="read write", resource_metadata="https://x/meta"',
  "Bearer realm=example, scope=single",
  'Basic realm="x"',
  "Bearer",
  'bearer error=invalid_token, error_description="expired token"',
  'Bearer resource_metadata="not a url"',
].map((header) => {
  const params = extractWWWAuthenticateParams(
    new Response(null, { status: 401, headers: { "WWW-Authenticate": header } }),
  );
  return {
    header,
    params: {
      resourceMetadataUrl: params.resourceMetadataUrl?.href ?? null,
      scope: params.scope ?? null,
      error: params.error ?? null,
      errorDescription: params.errorDescription ?? null,
    },
  };
});

const discoveryUrls = [
  "https://as.example",
  "https://as.example/",
  "https://as.example/tenant/x/",
  "https://as.example/tenant?y=1",
].map((url) => ({
  url,
  urls: buildDiscoveryUrls(url).map((entry) => ({ url: entry.url.href, type: entry.type })),
}));

const resources = [
  ["https://mcp.example/mcp#frag", "https://mcp.example/"],
  ["https://mcp.example/mcp", "https://mcp.example/mcp"],
  ["https://mcp.example/mcp", "https://mcp.example/mcp/"],
  ["https://mcp.example/mcpx", "https://mcp.example/mcp"],
  ["https://mcp.example/a/b", "https://mcp.example/a"],
  ["https://mcp.example/a", "https://mcp.example/a/b"],
  ["https://mcp.example:8443/a", "https://mcp.example/a"],
].map(([requested, configured]) => ({
  requested,
  configured,
  resource: resourceUrlFromServerUrl(requested).href,
  allowed: checkResourceAllowed({ requestedResource: requested, configuredResource: configured }),
}));

const authMethods = [
  [{ client_id: "c" }, []],
  [{ client_id: "c", client_secret: "s" }, []],
  [{ client_id: "c", client_secret: "s" }, ["client_secret_post"]],
  [{ client_id: "c", client_secret: "s" }, ["client_secret_basic", "client_secret_post"]],
  [{ client_id: "c" }, ["client_secret_basic"]],
  [{ client_id: "c" }, ["none", "client_secret_basic"]],
  [{ client_id: "c", client_secret: "s" }, ["private_key_jwt"]],
  [
    { client_id: "c", token_endpoint_auth_method: "client_secret_post", client_secret: "s" },
    ["client_secret_basic"],
  ],
  [{ client_id: "c", token_endpoint_auth_method: "none" }, []],
].map(([clientInformation, supported]) => ({
  clientInformation,
  supported,
  method: selectClientAuthMethod(clientInformation, supported),
}));

const clientMetadata = [
  ["http://127.0.0.1:5000/oauth/callback/mcp/x", {}],
  ["https://app.example/cb", { scope: "read", clientSecret: "s", clientName: "Mine" }],
].map(([redirectUrl, config]) => ({
  redirectUrl,
  config,
  metadata: resolveClientMetadata({
    redirectUrl,
    clientMetadata: {
      client_name: config.clientName ?? "ZCode srv",
      grant_types: ["authorization_code", "refresh_token"],
      redirect_uris: [redirectUrl],
      response_types: ["code"],
      ...(config.clientSecret ? { token_endpoint_auth_method: "client_secret_basic" } : {}),
      ...(config.scope ? { scope: config.scope } : {}),
    },
  }),
}));

const content = `${JSON.stringify(
  {
    keyPrefixes,
    pairInputs,
    scopeUnions,
    wwwAuthenticate,
    discoveryUrls,
    resources,
    authMethods,
    clientMetadata,
  },
  null,
  1,
)}\n`;
const target = new URL(
  "../apps/zcode-cli-rust/crates/domain/tests/fixtures/mcp_oauth_corpus.json",
  import.meta.url,
);
if (process.argv.includes("--check")) {
  const current = await readFile(target, "utf8").catch(() => "");
  if (current !== content) throw new Error("Rust MCP OAuth corpus differs from TS");
} else await writeFile(target, content);
