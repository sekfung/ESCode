# Rust Host `client-config` / `client-scenes` 域（U2 · P6）

> 状态：设计 · 2026-10-16。TS 真相源：`packages/services/src/client-config/clientConfigService.ts`、`client-scenes/clientScenesService.ts`、
> `providers/api/{nodeApiClient,apiJson,requestIdHeaders}.ts`、`providers/sourceHeaders.ts`；`@escode/shared` 的 `clientConfig.ts` / `pluginStoreOrder.ts` /
> `dynamic-workflow-feature.ts` / `escode-source-headers.ts` / `escodeEndpoint.ts`。上游：`rust-u2-providers.md` §12（P5b 的无凭据下载）。

## 1. 范围与决定

| 成员                                           | 决定                                                                                                              |
| ---------------------------------------------- | ----------------------------------------------------------------------------------------------------------------- |
| `client-scenes.list`                           | 迁。                                                                                                              |
| `client-config.getSnapshot`                    | 迁。                                                                                                              |
| `client-config.getDynamicWorkflowClientConfig` | 迁。                                                                                                              |
| `client-config.getOffPeakClientConfig`         | **已删除（2026-10-20）**。off-peak / 闲时任务功能整体下线，该成员从 TS 与 Rust 服务面同时移除，不再计入迁移缺口。 |

**ApiClient 由 Host 自己实现**（用户决定，2026-10-16）：用统一出口 `escode_cli_net`（`Purpose::WebFetch`，与 P5b、`system_probe` 同一入口），不经 Main 代理请求，
不新增控制消息。Main 只经 `rpcListen.runtimeEnv` 下发静态事实（见 §3）。

## 2. 所有者与事件顺序

```
Renderer ─client-scenes.list / client-config.get*→ Rust Host
                                   │ Endpoint 每次解析（P5b `EndpointSource`：env 基址 + 设置覆盖，只读设置文件）
                                   ▼
                   ApiClient：改写默认 Origin → 当前 Endpoint ▸ 注入来源头 ▸ 补 x-request-id ▸ 超时
                                   ▼
                 GET <endpoint>/api/v1/client/{configs|scenes}   （无凭据，不跟随重定向）
```

- **缓存的唯一所有者**：`ClientConfigService`（Host 进程内一份）。`getSnapshot` 按 URL（含 `app_version`/`platform`/endpoint）分条目缓存 1 h，同条目并发请求合并（in-flight）；
  `getDynamicWorkflowClientConfig` 用另一份“原始信封”缓存（同端点、同 TTL，解析口径不同，两份共存——与 TS 同）。`forceRefresh` 绕过 TTL。
- **缓存键含 Endpoint**：设置里切换 `escodeEndpointOrigin` 后下一次读取走新条目，不把新地址的响应写到旧条目（TS：“endpoint 可能在两次 await 之间变化”）。

## 3. ApiClient 规则（对照 `nodeApiClient.ts`）

1. **Endpoint 改写**（`rewriteESCodeEndpointUrl`）：URL 的 origin 等于默认 Origin（`https://zcode.z.ai`）且当前 Endpoint 不同 → 换成当前 Endpoint 的 origin（保留 path/search/hash）；其余 URL 原样。
2. **来源头**（仅当请求 origin 等于当前 Endpoint 时注入，调用方同名头优先）：`User-Agent: ESCode/<appVersion|unknown>`、`HTTP-Referer: <endpoint>`、`X-Title: ESCode@electron`、
   `X-ZCode-App-Version`、`X-Platform: <platform>-<arch>`、`X-Release-Channel`、`X-Client-Language`、`X-Client-Timezone`、`X-Os-Category`（darwin→macos、win32→windows、其余 linux）、`X-Os-Version`、`X-Device-Mid`。
   值经 `normalizeESCodeSourceHeaderValue`（trim；非可打印 ASCII → 视为缺省；`language/timezone` 缺省为 `unknown`）。
3. **`x-request-id`**：调用方没带就补一个 UUID v4。
4. **超时**：`timeoutMs` > 0 才计时；超时错误文案 `Request timed out after <n>ms`。
5. **错误**：非 2xx → `ApiError`（`message` 取响应正文 JSON 的 `error|message|msg|detail` 首个字符串，否则 trim 后的正文，否则 `HTTP <status>`；内部带 `url/method/status` 与 `x-request-id|x-trace-id|x-span-id` 响应头，跨 RPC 只剩 `status`，见 §6 ③）；
   网络错误/中止 → `ApiError`（无 status）。JSON 解析失败 → `ApiError`。（JWT/业务 401 观察器随 OAuth 删除而无调用方，不迁。）

**Main 下发的静态事实**（`runtimeEnv`，缺省值写在括号里）：`ESCODE_APP_VERSION`（P5 已有；缺省 `unknown`）、`ESCODE_ENV`（P5 已有，即 `releaseChannel`）、`ESCODE_CLIENT_LANGUAGE`（`unknown`）、
`ESCODE_CLIENT_TIMEZONE`（`unknown`）、`ESCODE_OS_VERSION`（缺省不带头）。`platform/arch` 由 Host 自算（`linux|darwin|win32` × `x64|arm64`，与 Node 同名）。
`X-Device-Mid` 读 `<appConfigDir>/telemetry-state.json` 的 `deviceMid`（只读；缺失不生成新身份），`appConfigDir` = P5 已有的 `ESCODE_BUILTIN_CACHE_ROOT`。

## 4. 方法语义

- **`client-scenes.list`**：`GET <runtime origin>/api/v1/client/scenes`，其中 `runtime origin` = `resolveRuntimeESCodeEndpointOrigin(进程 env)`（TS 在模块加载时取一次，**不**含设置覆盖），再经 §3.1 改写。
  应答 JSON 原样返回（TS 不校验），不缓存。
- **请求上下文**（TS `node.ts` 的 `resolveRequestContext`）：`app_version` = 应用版本，`platform` = **`${process.platform}-${process.arch}`**（如 `linux-x64`、`darwin-arm64`、`win32-x64`）——与 P5 下载用的 `linux-x86_64` 命名不同，不能混用。
- **`getSnapshot({forceRefresh?})`**：`forceRefresh` 经 `clientConfigReadOptionsSchema`（只允许布尔或缺省，其余键被剥掉）。请求 `<endpoint>/api/v1/client/configs?app_version=&platform=`（15 s 超时、`credentials: omit`、`redirect: error`），
  响应 `{code:0, data?:{configs?:{pluginStoreOrder?, zsrc?}|null}|null}`，否则 `Invalid public client config response`；`pluginStoreOrder` 经 `parsePluginStoreOrder`（宽容：坏的模式独立回退、列表去重、未知键剥掉），
  `zsrc.url` 须为 `http(s)` URL，否则 `null`。结果 `{pluginStoreOrder, zsrcUrl}`；返回副本。
- **`getDynamicWorkflowClientConfig({forceRefresh?})`**：用户模式（设置 `dynamicWorkflow` 用户选择，缺省 undefined）与进程 env `ESCODE_DYNAMIC_WORKFLOW_MODE`：env 覆盖合法即短路（不发请求）；
  否则读原始信封（共用 1 h 缓存）取 `data.configs.dynamicWorkflow`，经 `resolveDynamicWorkflowClientConfig` 折叠；请求失败 fail-closed（按“未下发”处理）并记 warn。折叠规则：覆盖 > 远端合法值 > `disabled`；
  功能未提供时用户选择不生效，可用时合法选择取代提供值。

## 5. 切片与验收

| 片     | 内容                                                                                         | 验收（TS 为 oracle）                                                                                                                                                   |
| ------ | -------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **S1** | Host ApiClient（§3）+ `client-scenes.list`                                                   | 本地 HTTP 桩：同一 URL 的请求头集合（归一化 `x-request-id`/`X-Device-Mid`）、路径与 query、错误形状与真实 `createClientScenesService` + `createNodeApiClient` 逐项相等 |
| **S2** | `getSnapshot`（含缓存/in-flight/forceRefresh/endpoint 切换）+ `pluginStoreOrder`/`zsrc` 解析 | TS 生成的解析黄金语料（同 P5b 的“配方”做法）；桩计数请求次数验证缓存与合并；endpoint 切换用 §P5b 的设置覆盖                                                            |
| **S3** | `getDynamicWorkflowClientConfig`（原始信封缓存、env 覆盖、用户模式、fail-closed）            | 同一桩上与真实 `createClientConfigService` 的返回逐项相等（覆盖 remote/override/default × userMode 的全组合）；桩宕机时 fail-closed                                    |

负向验证（每片）：改坏来源头注入条件、缓存键漏掉 endpoint、`credentials` 误带、env 覆盖不短路，对应用例失败。

## 6. 登记差异（不修）

1. 出口为 Rust 统一出口，代理/CA 取自 Host 网络配置（同 `rust-u2-providers.md` §12 ①）；3xx 以 `HTTP 3xx` 报错。
2. `Intl` 的 locale/时区由 Main 下发（Host 没有 `Intl`）；`X-Os-Version` 同。
3. **错误跨 RPC 的形状**：TS `ChannelServer` 只透传白名单键（`code/kind/status/retryAfterMs/data/detail/details/taskId/traceId`），所以 `ApiError` 到 Renderer 时只有 `name: "ApiError"`、`message`、`status`；`url/method/responseHeaders/cause` 在 TS 里同样到不了。Rust 以 `RpcError("ApiError", message).with("status", …)` 对齐，其余字段只进 Host 日志。
4. 不迁 JWT/业务 401 观察器；`getOffPeakClientConfig` 已随 off-peak 功能删除（§1）。
5. **JSON 解析错误的文案**：应答 2xx 但正文不是合法 JSON 时，TS 的 `ApiError.message` 是 V8 `JSON.parse` 的措辞（如 `Unexpected token '<', "<html>" is not valid JSON`），Rust 是 serde 的措辞；名字（`ApiError`）与 `status` 相同。逐字复刻 V8 的各类解析错误文案不值得，差分对这一类只比较名字与状态码。

## 7. 交付记录

**S1（2026-10-17）**：`host-services/src/api_client/`（`mod.rs` ApiClient：改写/来源头/request-id/超时；`source_headers.rs`；`transport.rs` 可注入传输 + `NetTransport`；`error.rs`）、
`client_scenes.rs`（含 `resolve_runtime_origin`）、`client_services.rs`（装配）、`host-rpc/src/domains_client.rs`（`client-scenes` 通道）；Main 侧 `buildRustHostClientEnv`（locale/时区/OS 版本）。
验收：`api_client` 8 项单测（改写向量 7 组、来源头、调用方头优先、错误文案 10 种、超时、非 Endpoint 的 Origin 只带 request-id）与 `runtime_origin` 向量 10 组，**均由真实 TS 运行得到**；
`rustHostClientScenes.test.ts`：本地 HTTP 桩 + 真实 `createClientScenesService`/`createNodeApiClient`，5 种桩行为（成功、业务错误码、HTTP 500、文本错误、坏 JSON）下应答/错误/请求路径/来源头集合逐项相等，无 Authorization/Cookie。
负向验证：去掉 `x-client-timezone` → 对拍失败。

**S2 + S3（2026-10-17）**：`host-services/src/client_config/`——`snapshot.rs`（`parse_snapshot` + `pluginStoreOrder`/`zsrc` 解析）、`service.rs`（按 URL 分条目的 1 h 缓存、`Shared` 合并在途请求、15 s 超时、
`clientConfigReadOptionsSchema`）、`dynamic_workflow.rs`（折叠规则纯函数）、`dynamic_service.rs`（原始信封缓存、env 覆盖短路、设置里的用户选择、fail-closed）；`host-rpc/src/domains_client.rs` 的 `client-config` 通道（2/3 个成员）。
验收：

- **TS 黄金语料**（`scripts/generate-escode-host-client-config-fixtures.mjs`，`--check` 已并入 `pnpm check:escode-host`）：97 个信封（含 36 种 `zsrc`、31 种 `pluginStoreOrder` 取值）与 **1408 种** 动态工作流“远端 × 覆盖 × 用户选择”组合，Rust 判定与真实 TS 逐条一致。
  负向验证（均被语料杀死）：`str::trim` 代替 JS 空白集、去掉去重、`zsrc` 接受任意 scheme、坏模式不作废；用户选择无视提供值、模式大小写不敏感、远端优先于覆盖。
- **服务层单测** 17 项：缓存与 TTL 边界、`forceRefresh`、并发合并（含 `forceRefresh` 加入在途请求）、失败共享且不入缓存、超时文案、Endpoint 切换用独立条目、选项校验、env 覆盖零请求、用户选择每次从设置读。负向验证：缓存键忽略 Endpoint、不合并在途请求、缓存失败，均被杀死。
- **端到端**：`rustHostClientConfig.test.ts`、`rustHostDynamicWorkflow.test.ts`——本地 HTTP 桩 + 真实 `createClientConfigService`，快照/请求路径与 query/来源头/错误类别逐项相等；覆盖远端 11 种取值、用户选择 × 提供值 10 种组合、503/非法信封/坏 JSON、env 覆盖短路（两边零请求）。负向验证：Host 无视用户选择 → 两个用例失败。

**补充登记差异**（接 §6）：6. **原始信封缓存不按 endpoint 分条目**（TS 原样如此，Rust 照做）：切换 Endpoint 后动态工作流读取仍用旧缓存直到 TTL/`forceRefresh`；快照缓存按 URL 分条目，不受影响。7. **`ZodError` 文案**：选项非法时两边都是 `ZodError`，措辞不同，只比类别。8. **“request context changed” 分支**：仅在“Endpoint 恰好是默认 Origin 且在两次解析之间被切走”时触发，难以确定性复现，只由代码对照 TS 保证，无单测。9. **off-peak 配置已删除（2026-10-20）**：`getOffPeakClientConfig`、`ESCODE_OFFPEAK_MOCK`、`ESCODE_OFFPEAK_MOCK_NO_PLAN` 与隐藏 Off-Peak Provider 均退出服务面。
