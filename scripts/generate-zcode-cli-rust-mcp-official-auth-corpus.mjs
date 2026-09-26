// Run with node --import tsx. 官方 MCP 鉴权纯规则的 TS oracle（docs/specs/rust-mcp-official-auth.md 第 1 期）：
// origin 信任判定（含 dev loopback）、保留头、ZCode API origin 解析、auth 配置严格解析。--check 防漂移。
import { readFile, writeFile } from "node:fs/promises";
import {
  findOfficialMcpReservedHeaders,
  isOfficialMcpOriginTrusted,
} from "../packages/shared/src/official-mcp-auth.ts";
import { resolveZCodeEndpointOrigin } from "../packages/shared/src/zcodeEndpoint.ts";
import { parseZCodeOfficialAuth } from "../apps/zcode-cli/packages/adapters/src/plugins/mcp-official-auth.ts";

const api = "https://zcode.z.ai";
const trust = [
  ["https://zcode.z.ai", undefined, api],
  ["https://zcode.z.ai:443", undefined, api],
  ["https://zcode.z.ai/mcp/path", undefined, api],
  ["https://user:pass@zcode.z.ai", undefined, api],
  ["http://zcode.z.ai", undefined, api],
  ["https://evil.example", undefined, api],
  ["https://ZCODE.Z.AI", undefined, api],
  ["  ", undefined, api],
  ["not a url", undefined, api],
  ["https://zcode.z.ai", undefined, undefined],
  ["https://zcode.z.ai", undefined, "http://zcode.z.ai"],
  ["http://127.0.0.1:3999", "http://127.0.0.1:3999", api],
  ["http://127.0.0.1:3999", " http://localhost:1 , http://127.0.0.1:3999 ", api],
  ["http://127.0.0.1:4000", "http://127.0.0.1:3999", api],
  ["http://localhost:3999", "http://localhost:3999", undefined],
  ["http://[::1]:3999", "http://[::1]:3999", api],
  ["https://evil.example", "https://evil.example", api],
  ["http://evil.example", "http://evil.example", api],
  ["http://u:p@127.0.0.1:3999", "http://127.0.0.1:3999", api],
].map(([origin, devTrustedOriginsRaw, zcodeApiOrigin]) => ({
  origin,
  devTrustedOriginsRaw: devTrustedOriginsRaw ?? null,
  zcodeApiOrigin: zcodeApiOrigin ?? null,
  result: isOfficialMcpOriginTrusted({
    origin,
    devTrustedOriginsRaw,
    pluginId: "p",
    zcodeApiOrigin,
  }),
}));

const reserved = [
  {},
  { "X-Custom": "1" },
  { Authorization: "x", "x-custom": "1" },
  { " MCP-Session-Id ": "s", "Bigmodel-Target-Type": "t", authorization: "a", AUTHORIZATION: "b" },
  { "x-coding-plan-api-key": "k", "X-Bigmodel-Authorization": "j", "bigmodel-project": "p" },
  { "Bigmodel-Organization": "o", "mcp-protocol-version": "v", "x-request-id": "r" },
].map((headers) => ({ headers, result: findOfficialMcpReservedHeaders(headers) }));

const origins = [
  [null, null],
  ["https://zcode.test.example/api/", null],
  ["  ", "http://127.0.0.1:8080/x"],
  [null, "https://Other.Example:443"],
  ["https://a.example", "https://b.example"],
].map(([envBaseOrigin, fallback]) => {
  const input = envBaseOrigin?.trim() ? envBaseOrigin : fallback;
  let result;
  try {
    result = { ok: resolveZCodeEndpointOrigin({ envBaseOrigin: input }) };
  } catch (error) {
    result = { error: String(error.message) };
  }
  return { baseUrl: envBaseOrigin, endpointOrigin: fallback, result };
});

const auth = [
  undefined,
  { type: "zcode_official", provider: "jwt_token" },
  { type: "zcode_official", provider: "jwt_token", extra: 1 },
  { type: "zcode-official", provider: "jwt_token" },
  { type: "zcode_official" },
  "zcode_official",
  null,
  [],
].map((value) => {
  let result;
  try {
    result = { ok: parseZCodeOfficialAuth(value, "plugin:server") ?? null };
  } catch (error) {
    result = { error: String(error.message) };
  }
  return { value: value === undefined ? "<undefined>" : value, result };
});

const content = `${JSON.stringify({ trust, reserved, origins, auth }, null, 1)}\n`;
const target = new URL(
  "../apps/zcode-cli-rust/crates/domain/tests/fixtures/mcp_official_auth_corpus.json",
  import.meta.url,
);
if (process.argv.includes("--check")) {
  const current = await readFile(target, "utf8").catch(() => "");
  if (current !== content) throw new Error("Rust MCP official auth corpus differs from TS");
} else await writeFile(target, content);
