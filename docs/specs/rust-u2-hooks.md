# Rust Host 阶段 2：hooks 域（rust-unified-host.md 子 spec）

> 状态：M3 本轮实现 · 2026-10-19
> 父计划：`rust-unified-host.md` §阶段 2；验收报告：`docs/reports/rust-migration-acceptance-2026-10-18.md` H2.6 / H3.4
> TS oracle（现场核对，本文件行号以 2026-10-19 检出的 `dev` 为准）：
>
> - `packages/services/src/hooks/hooksService.ts`：`loadHooksImpl:214-268`、`saveHooksImpl:287-308`、
>   user source `:105-120`、trust store 读取 `:154-212`、legacy source `:122-131`。
> - `packages/services/src/hooks/workspaceHookSettingsModel.ts`：`getWritableHook:70-91`、
>   `fromProjectSnapshot:161-193`、`fromUserESCodeSource:195-214`、`fromLegacyHooksConfig:216-249`、
>   `toESCodeHooksEvents:251-267`、`resolveNextRootEnabled:269-283`。
> - `packages/shared/src/workspace-hook-config.ts`：schema `:24-80`、runtime root `:130-152`、
>   gates `:154-167`、timeout `:113-127`、discovery `:207-221`、`readWorkspaceHookProjectSources:250-276`、
>   walk `:299-357`。
> - `packages/shared/src/workspace-hook-digest.ts`：entries `:62-133`、snapshot `:135-190`、
>   declaration digest `:192-261`。
> - `packages/shared/src/workspace-hook-trust-store-file.ts:20-99`（strict schema + 唯一键）。
> - `packages/shared/src/workspace-hook-mutation.ts:29-59`（同目录临时文件 + fsync + rename）。
> - `packages/services/src/escode-agent/escodeAgentService.ts:4258-4273`（grant 转发到 Agent authority）。
> - UI 调用：`packages/ui/src/store/hooksStore.ts:114,133,199,228`；
>   `packages/ui/src/settings/useWorkspaceHookInlineTrust.ts:90-131`；
>   远端链路 `packages/desktop/src/renderer/src/remoteWorkspaceSessionServices.ts:45-50`。

## 1. 范围与目标

把 Renderer 可达的 `hooks` channel 三个成员从 Pending 翻成 Implemented：

| 成员                      | Renderer 调用点                                                       | 目标语义                                                                    |
| ------------------------- | --------------------------------------------------------------------- | --------------------------------------------------------------------------- |
| `loadHooks`               | `hooksStore.ts:114,199,228`；`useWorkspaceHookInlineTrust` 信任后刷新 | 与 TS 同序发现项目/用户/legacy hooks，返回 `workspaceHookSnapshot` 与信任态 |
| `saveHooks`               | `hooksStore.ts:133`（增删改/启停都经它）                              | 与 TS 同文件同格式写 user 与 project 配置，保留未知键与键序                 |
| `grantWorkspaceHookTrust` | `useWorkspaceHookInlineTrust.ts:100-103`                              | 不在 Host 直接写 trust store；转发运行时 Agent authority 重新发现并授权     |

不在本轮：hooks 执行器与调用点（已在 `apps/escode-cli-rust`，见 `rust-hooks.md`）、删除 TS 代码、
`ESCODE_RUST_HOST` 开关、窗口 host 接线、其它 channel。

## 2. 状态所有者

- **Host 是 hooks 配置文件（`.escode/cli/config.json`、`<workspace>/.escode/config.json`）的唯一写者**。
  其他来源（`.agents/settings.json`、`.claude/settings.json`）只读。
- **workspace hook trust store 的唯一写者是运行时 Agent authority**（`workspace/hooks/trustGrant` →
  workflow host 的 `hooks.trustGrant`）。Host 只读 store 展示信任态；`grantWorkspaceHookTrust` 只做转发，
  不复制重新发现/写库逻辑（`IHooksService` 契约注释同义）。
- Host 内不维护 hooks 缓存：每次 `loadHooks` 重新读文件，与 TS 一致；文件由外部编辑后下一次读取可见。
- `workspaceIdentity` 只用于身份隔离（snapshot 的 `workspaceIdentity` 与 trust 记录过滤），
  `workspacePath` 用于发现、文件读写与路径展示；身份键为 `workspaceIdentity?.trim() || workspacePath`。

## 3. 持久化文件与格式

| 文件                                                                     | 读写 | 说明                                                                                                         |
| ------------------------------------------------------------------------ | ---- | ------------------------------------------------------------------------------------------------------------ |
| `<home>/.escode/cli/config.json`                                         | 读写 | 用户级 ESCode hooks；`saveHooks` 顶层合并，保留 `features`/`storage` 等未知键                                |
| `<workspace>/.escode/config.json`                                        | 读写 | 项目级 ESCode hooks；`editable` 仅此文件                                                                     |
| `<workspace>/escode.json`                                                | 只读 | 项目 hooks 候选（自动发现）                                                                                  |
| `<workspace>/.agents/settings.json`、`<workspace>/.claude/settings.json` | 只读 | legacy hooks（`enabled:false`、`editable:false`）                                                            |
| `<home>/.agents/settings.json`、`<home>/.claude/settings.json`           | 只读 | 用户 legacy hooks                                                                                            |
| `<storageRoot>/security/workspace-hook-trust-v1.json`                    | 读   | trust store；写者只有运行时。`storageRoot` = 用户配置 `storage.dir`（`~/`、绝对、相对）否则 `<home>/.escode` |

写入格式：`JSON.stringify(value, null, 2) + "\n"`，同目录临时文件 `0600` → fsync → rename（`host-core::atomic_write::write_secret`）。
键序按 JS 对象展开语义保留：已有顶层键顺序不变，`hooks`/`enabled`/`events` 已存在时保持原位置，否则追加。

`workspace-hook-trust-v1.json` 的 strict schema（与 `workspace-hook-trust-store-file.ts:37-75` 同）：

```json
{
  "schemaVersion": 1,
  "records": [
    {
      "workspaceIdentity": "…",
      "hookDeclarationDigest": "64 位小写 hex",
      "digestAlgorithm": "sha256",
      "decision": "trusted",
      "grantedAt": "ISO-8601",
      "lastUsedAt": "ISO-8601?",
      "bundleDigestAtGrant": "64 位小写 hex?",
      "eventAtGrant": "SessionStart|UserPromptSubmit|PreToolUse|PermissionRequest|PostToolUse|PostToolUseFailure|Stop",
      "displayCommandAtGrant": "非空",
      "sourcePathAtGrant": "非空",
      "sourceDiscoveryOrderAtGrant": 0,
      "matcherAtGrant": "string|null?",
      "matcherIndexAtGrant": 0,
      "hookIndexAtGrant": 0,
      "appVersionAtGrant": "非空?"
    }
  ]
}
```

- 顶层/记录对象都 strict：未知键、缺字段、非法枚举/时间戳、重复 `(workspaceIdentity, hookDeclarationDigest)`
  一律判 corrupt；`workspaceIdentity`/`displayCommandAtGrant`/`sourcePathAtGrant` 按 zod `.trim()` 后使用。
- 读取：ENOENT → 空集且 `corrupt:false`；其它 IO 错误、JSON 语法错误、schema 失败 → 空集且 `corrupt:true`
  （fail-closed，与 runtime/adapters 同判）。

## 4. 发现与装配语义

1. `workspacePath = resolve(params.workspacePath)`；`workspaceIdentity = trim(identity) || workspacePath`。
2. 项目配置发现：从 workspace 向上找 `.git`（文件或目录）标记；找到则根到叶、未找到只取 workspace 本身；
   每个目录候选 `escode.json`、`.escode/config.json`，按存在过滤、按规范化路径去重（保留首次）。
3. 读取项目源：JSON 非对象/读失败/`hooks` schema 失败 → 记入 `errors` 并跳过；`hooks` 缺失 → 静默跳过。
   `loadHooks` 与 TS 一样忽略 `errors`。
4. 用户源：`<home>/.escode/cli/config.json` 的 `hooks`，schema 失败 → 无用户源。
5. `runtimeRoot = resolveWorkspaceHookRuntimeRoot([userHooks, ...projectHooks])`：`enabled` 任一为 true；
   `timeoutMs`/`maxOutputBytes` 后者覆盖前者，缺省 60000 / 32768。
6. 每个声明计算 `configuredEnabled = (source.enabled !== false) && (declaration.enabled !== false) && runtimeRoot.enabled`。
7. `workspaceHookSnapshot` 只含项目源；没有项目 hook 时省略。`discoveredAt` 为当前 UTC ISO；
   `bundleDigest`/`hookDeclarationDigest` 用 `sha256(JSON.stringify(payload))` 逐字复刻（数组定序、`null` matcher、
   `["unset"]`/`["set", value]` 编码）。
8. Hook 列表顺序：项目 snapshot → 项目 legacy agents → 项目 legacy claude → 用户 ESCode →
   用户 legacy agents → 用户 legacy claude。legacy id 为 `hook-<source>-<scope>-<counter>`，只统计有效声明。
9. `hooksEnabled = hooks.some(enabled)`；项目/用户 hook 的 `enabled` 是 `configuredEnabled`（含 runtimeRoot 开关）。
10. trust 态：项目 hook 的 `workspaceHook.trustState` = 持久 digest 命中 ? `trusted_persistent` : `pending_trust`。

`loadHooks` 结果：

```ts
{
  hooks: Hook[];
  hooksEnabled: boolean;
  workspaceHookSnapshot?: WorkspaceHookBundleSnapshotData;
  trustStoreCorrupt?: true;
}
```

## 5. 保存语义

`saveHooks({workspacePath, workspaceIdentity?, hooks})`：

- user 集合：`editable !== false` 且（无 `location` 或 `location.source === "escode" && location.scope === "user"`）。
- project 集合：`editable !== false` 且 `location.source === "escode"`、`location.scope === "project"`，
  且无 `configuredState` 或 `resolve(configuredState.sourcePath) === resolve(workspacePath, ".escode/config.json")`。
- 分别写 user、project 两份配置；每份都先读旧对象（非对象/坏 JSON → `{}`），再：
  - `enabled = resolveNextRootEnabled(existing.hooks.enabled, hooks)`：任一 hook `enabled === true` 且
    （无 `configuredState` 或 `enabled !== configuredState.configuredEnabled`）→ `true`，否则保留旧值。
  - `events = toESCodeHooksEvents(hooks)`：按 hook 顺序分组；matcher 相同（含都为空）复用；
    writable hook 先展开 `custom`，再写 `type`/`command`/`enabled`/`statusMessage`，command 另写
    `async`/`shell`/`timeout`（秒），process 另写 `args`（非空）/`timeoutMs = timeout*1000`。
- 没有 hook 时仍写 `hooks.events = {}`（不清除旧 `enabled`/`timeoutMs`/`maxOutputBytes`）。

## 6. 接口与错误

| 方法                      | 参数                                                                       | 结果                                                | 错误                                                                               |
| ------------------------- | -------------------------------------------------------------------------- | --------------------------------------------------- | ---------------------------------------------------------------------------------- |
| `loadHooks`               | `{workspacePath, workspaceIdentity?}`                                      | 上述结果                                            | 缺/非绝对 `workspacePath` → `InvalidRequest`；文件读取失败按 TS 语义降级（不会抛） |
| `saveHooks`               | `{workspacePath, workspaceIdentity?, hooks}`                               | `undefined`                                         | IO/schema 失败 → `Error`，message 保留底层原因                                     |
| `grantWorkspaceHookTrust` | `{workspacePath, workspaceIdentity?, bundleDigest, hookDeclarationDigest}` | `{accepted:true}` 或 `{accepted:false, reasonCode}` | 缺参数 → `InvalidRequest`；运行时错误按 `ESCodeProtocolClientError` 透传           |

`grantWorkspaceHookTrust` 的 reasonCode 由 Agent authority 产生（`workspace_hooks_policy_requires_pretrust`、
`workspace_hooks_blocked_by_policy`、`workspace_hooks_bundle_changed`、`workspace_hooks_trust_store_corrupt`、
`workspace_hooks_config_unreadable` 等）；Host 不改写、不把底层异常文本塞进 reasonCode。

## 7. 事件顺序（无推送）

```
Renderer                    Host (host-rpc)                    FS / Runtime
   │ loadHooks(req)             │                                  │
   ├───────────────────────────►│ 解析 workspacePath/identity       │
   │                            ├─ 发现项目源（向上找 .git）────────►│
   │                            ├─ 读用户源 ───────────────────────►│
   │                            ├─ 计算 runtimeRoot/entries/digests │
   │                            ├─ 读 trust store（fail-closed）───►│
   │◄───────────────────────────┤ {hooks,hooksEnabled,snapshot?,corrupt?}
   │                            │                                  │
   │ grantWorkspaceHookTrust    │                                  │
   ├───────────────────────────►│ runtime_target/workspace ref      │
   │                            ├─ workspace/hooks/trustGrant ─────►│ Agent authority
   │                            │                                  ├─ 重新发现 canonical snapshot
   │                            │                                  ├─ 写 trust store（原子）
   │◄───────────────────────────┤ {accepted,...}                   │
```

本域没有订阅/流/事件推送，因此 `desktop-continuous` 与 `web-remote-replayable` 的 stream/snapshot/queue
差异不适用；两种连接路径都必须能完成上述请求/应答（由真实客户端用例覆盖）。

## 8. 已登记差异

| #   | 差异                                                                                                                                               | 原因                                                         | 移除条件                                                                            |
| --- | -------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------ | ----------------------------------------------------------------------------------- | --------------------------------------------------------------- |
| 1   | 相对 `workspacePath` 被拒绝（`InvalidRequest`），TS 按进程 cwd `resolve`                                                                           | Host 不读进程 cwd；业务路径由 Main/Renderer 提供且为绝对路径 | Host 协议增加 `cwd` 事实并显式传入                                                  |
| 2   | `workspaceHookSnapshot.discoveredAt` 取 Host 当前时间，与 TS 调用时刻不同                                                                          | 快照生成时间天然不同，且不参与任何 digest                    | 不需要移除；差分只比较除该字段外的结构与 digest                                     |
| 3   | trust store 时间戳校验只接受 RFC3339 子集 `YYYY-MM-DDTHH:MM:SS[.fraction](Z                                                                        | ±HH:MM)`                                                     | 不引入日期库；zod `.datetime()` 接受的范围更大（如纯日期时间无时区在某些 zod 版本） | 出现 TS 写入且 Host 判 corrupt 的真实夹具时收紧/放宽到 zod 口径 |
| 4   | 旧配置里 `hooks` 为非对象时按空对象处理；JS 展开字符串会得到字符下标键                                                                             | 损坏配置不应被“字符键”放大；两种都不可被运行时 schema 接受   | 不再需要；保持 fail-closed                                                          |
| 5   | `grantWorkspaceHookTrust` 需要运行时与 Node workflow host（`ESCODE_PLUGIN_HOST_*`）；不可用时返回错误而不是本地写库                                | 信任库唯一写者是 Agent authority（`IHooksService` 契约）     | 运行时提供不依赖 Node 的 native trust authority                                     |
| 6   | `saveHooks` 顶层/`hooks` 对象键序按 JS 展开保留；数组/值逐字一致                                                                                   | 持久化格式 L5 要求                                           | 不需要移除                                                                          |
| 7   | legacy hooks 的 `args`/`async`/`shell` 按 TS 原样保留（不因 type 丢弃）                                                                            | TS `fromLegacyHooksConfig` 同形                              | 不需要移除                                                                          |
| 8   | Renderer 传入 Hook 的 `custom` 等 passthrough 对象在 Host RPC 解码时键序变为字典序（`Wire::Object` 是 `serde_json::Value`）；TS 保留请求对象插入序 | RPC 线上对象以 JSON 文本传输，Host 现有解码器不保序          | `Wire` 的对象表示改为保序容器（影响全部 channel，单独评估）                         |

## 9. 验收

- `hooks` 3/3 在 `inventory_services.rs` 翻为 Implemented，`contract.rs` 3 例与 `inventory --check` 通过。
- Rust 单测：discovery 顺序/去重、schema 严格性、digest 与 TS 冻结值逐字一致、save 键序/未知键、
  trust store corrupt/ENOENT/过滤、legacy 解析。
- TS oracle 差分（L4）：用真实 `createHooksService()` 与 Rust `loadHooks` 对同一临时目录做结构全等
  （除 `discoveredAt`）；`saveHooks` 后两份落盘文件与 TS 输出逐字节一致（L5）。
- 真实客户端：`@escode/client` 经 `@escode/rpc` 调用三个成员；`loadHooks`/`saveHooks` 成功路径 +
  非绝对路径错误；`grantWorkspaceHookTrust` 在 workflow host 可用时成功、缺失参数时失败。
- 存量读入：TS 写出的 `config.json`、legacy `settings.json`、trust store 夹具由 Rust 读入。
- 负向验证：独立 worktree 删除 `hooks` 的 Implemented 登记/方法表项，确认契约测试失败。
