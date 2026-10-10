# Rust 模型请求头与 Node 对齐

2026-10-08。对照 MBearo/ESCode-rs 的 `net` crate（device / gateway 头）核对发现：Node 每次模型请求都带一组客户端头与
归因头，Rust 只发 `authorization` / `content-type` / `accept`。这些头供网关做来源归因、会话追踪与统计
（Coding Plan 按 `x-escode-session-type` 区分 main / subagent / other），缺失会让 Rust 的请求在服务端无法归因。

App 差分实测（同一 Registry 夹具）Node 的请求头：`http-referer`、`user-agent`、`x-client-language`、`x-client-timezone`、
`x-os-category`、`x-os-version`、`x-platform`、`x-release-channel`、`x-title`、`x-escode-agent`、`x-escode-app-version`、
`x-escode-session-type`、`x-escode-trace-id`、`x-session-id`、`x-query-id`、`x-request-id`。

## 规则

### 静态客户端头（TS `bootstrap/src/model-config.ts` buildCliESCodeSourceHeaders，进程内不变）

| 头                    | 值                                                                                                       |
| --------------------- | -------------------------------------------------------------------------------------------------------- |
| `http-referer`        | `ESCODE_BASE_URL` ?? `ESCODE_ENDPOINT_ORIGIN` 的 origin，缺省 `https://zcode.z.ai`                         |
| `user-agent`          | `ESCode/{appVersion}`                                                                                     |
| `x-escode-app-version` | `ESCODE_APP_VERSION`（Host 给两个 runtime 都注入）；缺省时 Node 用 CLI 自身版本，Rust 用 runtime 自身版本 |
| `x-title`             | `Z Code@electron`（argv 含 `app-server` / `agent-server`），否则 `Z Code@cli`                            |
| `x-release-channel`   | `ESCODE_ENV` 为 `test` → `test`，否则 `production`                                                        |
| `x-client-language`   | 进程默认 locale（BCP 47，如 `zh-CN`），不可打印时 `unknown`                                              |
| `x-client-timezone`   | 本机 IANA 时区（如 `Asia/Shanghai`），不可打印时 `unknown`                                               |
| `x-escode-agent`       | `glm`                                                                                                    |
| `x-platform`          | `{process.platform}-{os.arch}`（`win32-x64`、`darwin-arm64`、`linux-x64`）                               |
| `x-os-category`       | `windows` / `macos` / `linux`                                                                            |
| `x-os-version`        | `os.release()`：Windows `10.0.{build}`，Unix 为内核版本                                                  |

值须为可打印 ASCII（`normalizePrintableHeaderValue`），否则省略（locale / 时区回落 `unknown`）。

### 每次请求的归因头（TS `adapters/src/model/runner-attribution.ts`，覆盖同名静态头）

| 头                     | 值                                                                                 |
| ---------------------- | ---------------------------------------------------------------------------------- |
| `x-request-id`         | 每次 HTTP 尝试新生成的 UUID（重试不复用）                                          |
| `x-escode-session-type` | 主代理轮 `main`，子代理 `subagent`，其余（压缩、标题、记忆提取、工作流等）`other` |
| `x-escode-trace-id`     | 会话 trace id                                                                      |
| `x-session-id`         | 会话 id 去掉内部前缀 `sess_`、`subagent_agent_`                                    |
| `x-query-id`           | 本轮 query id（去掉 `query_` 前缀），缺失时省略                                    |
| `x-opencode-session`   | 仅 base URL 为 `*.opencode.ai/zen/go/v1` 时，值同 `x-session-id`                  |

### 合并顺序

静态客户端头 → Provider 配置头 → 归因头 → 鉴权（`authorization` / `x-api-key` 与 Host 每请求鉴权头）。同名（不区分大小写）
后者覆盖前者，不产生重复头。

## 所有者

- `escode_cli_host::client_platform`：平台事实（platform / arch / os 版本 / locale / 时区），进程内解析一次。
- `escode_cli_model::client_headers`：静态头与归因头的组装；归因来源为 `core_api::model_call::ModelCallScope`
  （新增 `trace_id`，由 run 作用域填入会话 trace id）。

## 已知差异

- `user-agent`：Node 由 ai-sdk 追加 `ai-sdk/provider-utils/{ver} runtime/node.js/{ver}` 后缀，Rust 只发配置值 `ESCode/{ver}`，
  不伪造 Node 运行时标识。
- `x-query-id`：Node 取 traceContext 的 queryId，Rust 取本轮 turn id（同为每轮唯一）；两侧取值来源不同，差分只比格式。
- `accept` / `accept-language` / `sec-fetch-mode`：Node fetch（undici）自带，非产品语义，不对齐。
- POSIX locale 取 `LC_ALL` → `LC_MESSAGES` → `LANG`（ICU 同序），`C` / `POSIX` 视为 `en-US`。
- 时区：V8 把 UTC 的各种别名（`Etc/UTC`、`GMT`、`Zulu` 等）归一为 `UTC`，Rust 同样归一（三平台 CI 的 runner 时区为 `Etc/UTC`）；
  ICU 其余旧链接名的改写（如 `US/Pacific` → `America/Los_Angeles`）未对齐，系统时区通常已是规范名。

## 验收

- 单测：前缀剥离、会话类型映射、OpenCode 判定、合并去重覆盖、可打印校验、locale 规范化。
- App 差分：`escode-cli-rust-model-headers.test.ts`（同一夹具与环境，两侧静态头逐字一致；归因头的键集合一致、
  会话类型 / trace / session 值与格式一致；重试时 `x-request-id` 变化）。
