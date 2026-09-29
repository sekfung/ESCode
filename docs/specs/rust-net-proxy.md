# Rust 网络代理解析与 TS 对齐（WP4）

2026-09-24。TS 的模型与工具请求都经 `adapters/src/network/http-config.ts` 显式解析代理；Rust 之前直接用 reqwest 客户端，
只认 `HTTP(S)_PROXY`，因此同一台机器上两个 runtime 的出口可能不同。本规格把解析逻辑做成 domain 内的纯函数，
并按同一份判定接入模型客户端。

## 规则（与 TS 逐条一致）

| 顺序 | 来源                                                                                     | 结果                                                 |
| ---- | ---------------------------------------------------------------------------------------- | ---------------------------------------------------- |
| 1    | 非 http/https 目标                                                                       | 不使用代理（`noProxyMatched=false`）                 |
| 2    | 显式 `noProxy` 或 `ZCODE_NO_PROXY` 命中                                                  | 绕过代理（`noProxyMatched=true`）                    |
| 3    | 显式 `httpProxy`                                                                         | 代理来源 `network.httpProxy`                         |
| 4    | `ZCODE_HTTP_PROXY`                                                                       | 代理来源 `env:ZCODE_HTTP_PROXY`                      |
| 5    | 工具运行请求（web-fetch 变体）：捕获环境里的 `no_proxy`/`NO_PROXY` 命中                  | 绕过代理                                             |
| 6    | 捕获环境的 `https_proxy`→`HTTPS_PROXY`→`http_proxy`→`HTTP_PROXY`→`all_proxy`→`ALL_PROXY` | 代理来源 `env:ZCODE_TOOL_ENV_PASSTHROUGH_JSON.<key>` |
| 7    | 其余                                                                                     | 不使用代理                                           |

- 代理值缺 scheme 时按 `http://` 补齐；解析失败视为无值。`noProxy` 支持 `*`、`example.com`、`.example.com`、`*.example.com`、
  `host:port`（端口不符则不匹配）、`[ipv6]`（忽略端口）、带 scheme 的完整 URL；匹配为后缀式（`host` 及其子域）。
- 捕获环境来自 `ZCODE_TOOL_ENV_PASSTHROUGH_JSON`（JSON 对象，仅字符串值，键名须匹配 `^[A-Za-z_][A-Za-z0-9_]*$`）。

## 已知差异

- Host 侧的环境捕获过滤（`shouldCaptureZCodeToolEnvPassthroughKey` 的排除名单）未移植：Rust 只消费已捕获的键值，
  不产出该变量，因此不会把不该捕获的键写进去。
- Host 下发配置（`network.httpProxy` / `noProxy` / CA）尚未接入：Rust 目前的 `httpProxy`/`noProxy` 恒为 None，
  即只走环境变量来源。接入时填充 `ProxyOptions` 的这两个字段即可，判定不需改动。
- CA 文件已接入（2026-09-30）：`host::tls_ca` 读 `ZCODE_AGENT_CA_CERT`（显式来源，读不到即失败，
  与 TS `readFileSync` 同路），缺席时回落 `NODE_EXTRA_CA_CERTS`（Node 原生行为：文件不存在只忽略）；
  模型客户端与 WebFetch 客户端把 PEM（可含多张）/DER 证书加进根证书。设置页注入时两个变量都在
  （`services/src/runtime-tools/agentProxyEnv.ts`），独立 CLI 只设后者——两条都要认。
- MCP 客户端（`mcp_hub.rs`）仍用 reqwest 默认行为，待与 TS MCP 传输路径一并核对（同下一条）。
- MCP 网络已接入（2026-09-30）：`tools/src/mcp_http.rs` 按目标 URL 解析代理（显式/`ZCODE_HTTP_PROXY`/
  `ZCODE_NO_PROXY`/捕获环境）并信任自定义 CA；MCP streamable HTTP / SSE 传输按 URL 缓存客户端，
  OAuth 发现与令牌端点（`mcp_oauth_http.rs`，整请求 60s）走同一条构造。TS 侧是 `createMcpTransportFetch`
  的 per-request fetch，两侧判定同源。

## 验收

1. 差分：`scripts/generate-zcode-cli-rust-proxy-corpus.mjs` 以 TS 两个入口为 oracle，导出
   2,464 条（url × 17 种显式配置 × 14 种环境变量），Rust 逐条比对 `noProxyMatched`/`proxySource`/`proxyUrl`。
2. 实机：设置 `ZCODE_HTTP_PROXY` 后请求走代理、`ZCODE_NO_PROXY` 命中时直连；未设变量且无捕获环境时不影响本地地址。
3. 差分（2026-09-30）：`packages/services/tests/zcode-cli-rust-network.test.ts` 的
   「settings-page CA is trusted by Node and Rust」——同一份 CA→叶子证书链的本地 TLS 模型服务，
   Node 带 `NODE_EXTRA_CA_CERTS`+`ZCODE_AGENT_CA_CERT`、Rust 只带 `ZCODE_AGENT_CA_CERT`，两者都必须
   成功收到模型回复（叶子不能是 `CA:TRUE`：rustls 明确拒绝把 CA 当服务端实体，Node/OpenSSL 宽容——
   单张自签证书会造出假差异）。
4. 差分（2026-09-30）：`packages/services/tests/zcode-cli-rust-mcp.test.ts` 的
   「MCP HTTP transport goes through the configured proxy in both runtimes」——本地 forward 代理统计
   MCP 路径请求，Node 与 Rust 的 MCP 握手都必须经代理过去（模型 fixture 由 `ZCODE_NO_PROXY` 绕过）。
