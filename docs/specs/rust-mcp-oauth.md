# Rust MCP OAuth（授权码 + PKCE）

2026-09-26。承接 rust-mcp-parity.md 第 3 期。用户确认按本文实施（与 Node 共用加密凭据存储）。

进度：

- 第 1 层（已完成）：共享凭据存储。`crates/host/src/{credential_cipher,file_lock,credential_store}.rs`
  对齐 TS cipher、`atomicFileLock` 协议与 `shared-credentials.ts`；`zcode-cli-rust-credentials.test.ts` 验证默认 secret 推导、
  双向解密、同一路径与跨进程锁（Node/Rust 交错独立写入各 25 次无丢失；去掉 Rust 锁时该用例稳定失败）。
- 第 2 层（进行中）：OAuth 流程（discovery、DCR、PKCE、回调、刷新、租约）与 `mcp/list` 授权状态。

TS 基线：

| 模块           | TS 文件                                                       |
| -------------- | ------------------------------------------------------------- |
| OAuth 流程     | `adapters/src/mcp/oauth-*.ts`                                 |
| 本地回调       | `adapters/src/auth/localhost-callback.ts`                     |
| 凭据存储与加密 | `adapters/src/auth/{shared-credentials,credential-cipher}.ts` |
| 文件锁         | `@zcode/shared/node` `withFileLock` / `atomicFileLock`        |
| 协议           | `mcpServerConfig.oauth`、`mcp/list` 的 `authorization` 状态   |

## 目标

- 需要 OAuth 的远程 MCP server 在 Rust runtime 下可用。
- 凭据与 Node 共用同一份加密存储：
  - 任一 runtime 授权后，另一方直接复用；
  - 回退到 Node 不需要重新授权；
  - 两个 runtime 并发刷新时不互相作废 token。

## 所有者与流程

```mermaid
sequenceDiagram
  participant UI as Desktop 设置页
  participant H as Host
  participant R as Rust runtime（MCP hub）
  participant S as 授权服务器 / MCP server
  UI->>H: mcp/list（connect）
  H->>R: mcp/list
  R->>S: 连接；无凭据或 401
  R->>R: discovery + DCR + PKCE（rmcp auth AuthorizationManager），起 127.0.0.1 回调监听
  R-->>H: status.authorization = {type: oauth_authorization_code, authorizationUrl, startedAt}
  UI->>H: 轮询 mcp/list（mode=status）
  UI->>UI: 打开 authorizationUrl
  S-->>R: 回调 ?code&state（回调监听）
  R->>S: 换 token（PKCE verifier）
  R->>R: 写入共享凭据（文件锁内）
  R->>S: 以 Bearer 重连，状态转 connected
```

- 运行期 token：每次请求前读取共享凭据，临期时在跨进程锁内刷新，与 TS `refreshMcpOAuthTokensUnderLock` 相同。
- 401 处理：
  - 有 refresh token 时刷新一次后重试；
  - 否则把状态置为需要交互授权，与 TS `createInteractiveAuthorizationRequiredError` 相同。

## 兼容点（与 Node 共享的事实）

1. 凭据文件：路径同 TS `resolveSharedZCodeCredentialsPath`（`ZCODE_DATA_BASE_DIR` 或默认位置）。
   - 条目值以 `enc:v1:<iv>.<tag>.<cipher>` 形式保存，均为 base64url；
   - 算法为 AES-256-GCM，密钥为 `sha256(ZCODE_CREDENTIAL_SECRET 或 "zcode-credential-fallback:<platform>:<home>:<user>")`；
   - `<platform>` 取 Node `os.platform()` 的取值（`win32`/`darwin`/`linux`）。
2. 键：
   - 前缀为 `mcp:oauth:<sha256(serverName\nserverUrl\nclientId\nscope\nredirectPath) 前 24 位>`；
   - canonical 记录的键为 `<前缀>:authorization_credentials`；
   - 记录为 version 2 JSON：`client_information`、`tokens`、`published_by`、`generation`、`obtained_at`、`expires_at`、`issuer`；
   - legacy 键 `client_information`、`tokens` 只读兼容。
3. 文件锁：对 `<file>.lock` 做 mkdir，内含 `owner-<token>.json`（pid、createdAt、token）；陈旧锁与宽限期规则逐条对齐 `atomicFileLock.ts`。
4. 写入：原子写（临时文件 + rename），文件权限 0600。
5. 回调：
   - 监听 127.0.0.1 随机端口，路径为 `redirectPath`，缺省时按 server 名生成；
   - 成功/失败页文案、`state` 校验、授权服务器 `error` 回传的分类，均与 `localhost-callback.ts` 相同。
6. 授权事务有效期 5 分钟（`MCP_OAUTH_AUTHORIZATION_TRANSACTION_TTL_MS`）。
7. 多个进程同时授权时：租约内先到者主导，其余跟随并等待 canonical 换代（`oauth-lease.ts`）。

## 实现选择

- OAuth 协议部分使用 rmcp `auth` feature（AuthorizationManager：metadata discovery、DCR、PKCE、换 token、刷新），`CredentialStore`/`StateStore` 自实现为上述共享存储。
- 加密使用已随 rustls 引入的 `ring`（AES-256-GCM），不新增依赖；锁、原子写与存储在 host crate（domain 不做 IO）。
- 锁实例观察（TS `lockInstanceObserver`）在时间戳不可用时以首次观察时刻近似。

## 验收

- 语料（TS oracle）：
  - 凭据加解密与 Node 互读（同一 secret 下两侧加密的值互相解密）；
  - 键前缀 hash；
  - canonical 记录读写。
- 并发：Node 与 Rust 同时持锁刷新时只有一方换代，另一方读到新 generation。
- App 差分：本地授权服务器 fixture（discovery、DCR、authorize 重定向、token、refresh）上，Node 与 Rust 需要满足：
  - `mcp/list` 状态序列一致；
  - 回调后连接成功；
  - 两侧可用对方写入的凭据直接连接。
- 已知限制：
  - `official-auth.ts`（官方账号授权）需要 Host 账号能力，单独评估；
  - enterprise-managed 授权不在本期范围。
