// Run with node --import tsx. 以 TS resolveProxyForRequest / resolveWebFetchProxyForRequest 为 oracle，
// 导出 Rust 端代理解析的差分语料（环境变量、显式配置、noProxy 语法与失败回退）。
import { readFile, writeFile } from "node:fs/promises";
import {
  resolveProxyForRequest,
  resolveWebFetchProxyForRequest,
} from "../apps/zcode-cli/packages/adapters/src/network/http-config.ts";

const PASSTHROUGH = "ZCODE_TOOL_ENV_PASSTHROUGH_JSON";
const urls = [
  "https://api.example.com/v1/messages",
  "http://api.example.com/v1",
  "https://api.example.com:8443/x",
  "https://sub.deep.example.com/x",
  "https://example.com./x",
  "https://[2001:db8::1]/x",
  "https://127.0.0.1:3000/v1",
  "http://localhost:8080",
  "ftp://files.example.com/x",
  "not a url",
  "wss://socket.example.com",
];
const options = [
  {},
  { httpProxy: "http://explicit:8080" },
  { httpProxy: "explicit:8080" },
  { httpProxy: "   " },
  { httpProxy: "not a proxy" },
  { noProxy: "example.com" },
  { noProxy: "*.example.com" },
  { noProxy: ".example.com" },
  { noProxy: "other.com, example.com:8443" },
  { noProxy: "example.com:443,deep.example.com" },
  { noProxy: "example.com:99" },
  { noProxy: "*" },
  { noProxy: "https://example.com" },
  { noProxy: "[2001:db8::1]" },
  { noProxy: " api.example.com , " },
  { httpProxy: "http://explicit:8080", noProxy: "api.example.com" },
];
const envs = [
  {},
  { ZCODE_HTTP_PROXY: "http://env-proxy:3128" },
  { ZCODE_HTTP_PROXY: "env-proxy:3128" },
  { ZCODE_HTTP_PROXY: "" },
  { ZCODE_NO_PROXY: "example.com" },
  { ZCODE_HTTP_PROXY: "http://env-proxy:3128", ZCODE_NO_PROXY: "api.example.com,other:8080" },
  { ZCODE_HTTP_PROXY: "http://env-proxy:3128", ZCODE_NO_PROXY: "*" },
  { ZCODE_NO_PROXY: "api.example.com" },
  { [PASSTHROUGH]: JSON.stringify({ https_proxy: "http://captured:8080" }) },
  { [PASSTHROUGH]: JSON.stringify({ no_proxy: "api.example.com" }) },
  { [PASSTHROUGH]: JSON.stringify({ NO_PROXY: "example.com", http_proxy: "captured:8080" }) },
  { [PASSTHROUGH]: JSON.stringify({ all_proxy: "socks5://captured:1080" }) },
  { [PASSTHROUGH]: "not json" },
  { [PASSTHROUGH]: JSON.stringify({ "bad key!": "x", https_proxy: "http://captured:8080" }) },
];

const cases = [];
for (const url of urls) {
  for (let oi = 0; oi < options.length; oi += 1) {
    for (let ei = 0; ei < envs.length; ei += 1) {
      const options_ = { ...options[oi], env: envs[ei] };
      cases.push([
        url,
        oi,
        ei,
        resolveProxyForRequest(url, options_),
        resolveWebFetchProxyForRequest(url, options_),
      ]);
    }
  }
}
// 生成端自检：确保语料确实覆盖到有代理/被绕过/无代理三类结论。
const outcomes = new Set(cases.map((c) => JSON.stringify([c[3], c[4]])));
if (outcomes.size < 5) throw new Error("corpus does not cover enough outcomes");

const content = `${JSON.stringify({ urls, options, envs, cases })}\n`;
const target = new URL(
  "../apps/zcode-cli-rust/crates/domain/tests/fixtures/proxy_corpus.json",
  import.meta.url,
);
if (process.argv.includes("--check")) {
  if ((await readFile(target, "utf8")) !== content) {
    throw new Error(
      "Proxy corpus differs from TS; run node --import tsx scripts/generate-zcode-cli-rust-proxy-corpus.mjs",
    );
  }
} else {
  await writeFile(target, content);
}
