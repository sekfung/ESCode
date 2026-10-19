# Rust Host 阶段 2：bots 域（rust-unified-host.md 子 spec）

> 状态：M3 配置/状态子集 · 2026-10-19
> TS oracle：`packages/services/src/bots/botsService.ts`、`repo.ts`、`config.ts`；
> 平台流语义见 `docs/specs/bots-v4-stream.md`、`bot-channel-runtime-split.md`。
> UI：`packages/ui/src/BotsDialog.tsx`、`hooks/useBotGroupTask.ts`、`Root.tsx:386`。

## 1. 范围与状态

| 成员                                           | 状态                  | 说明                                                                                                                                                            |
| ---------------------------------------------- | --------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `getConfig` / `saveConfig` / `listBots`        | Implemented           | `<dataBaseDir>/.escode/v2/bot-config.v3.json`                                                                                                                   |
| `saveBot` / `removeBotSecret` / `deleteBot`    | Implemented（本地轴） | 配置 upsert/清引用/删除；不解析平台名称、不校验 provider 字段                                                                                                   |
| `getStatus` / `getBotStates` / `resetBotState` | Implemented           | 配置计数 + `bot-state.v3.json` 投影/清理                                                                                                                        |
| `createBindCode`                               | Implemented（内存）   | 6 位 hex、默认 30s TTL；绑定消费依赖入站消息（未迁移）                                                                                                          |
| `listWorkspaceRefs`                            | Implemented           | currentWorkspace + `setting.json.lastWorkspaceSession`                                                                                                          |
| `syncAppRuntimePreferences`                    | Implemented（no-op）  | Host 没有 Bot 远端 runtime；不新建连接                                                                                                                          |
| 其余 20 个成员                                 | **Pending**           | 飞书/微信注册轮询、Telegram/Webhook/飞书 provider、入站消息、回调、群成员/群任务、投递重试、topic resource/details、testBot、watchAutomationRun、replyToChannel |

## 2. 状态所有者与持久化

- 唯一写者：Host（App 级）。文件：
  - `<dataBaseDir>/.escode/v2/bot-config.v3.json`：`{version:3, bots: BotConfig[]}`；
  - `<dataBaseDir>/.escode/v2/bot-state.v3.json`：`{version:3, bots: Record<contextKey, BotContextState>}`。
- 写入：2 空格 JSON + 尾换行，0600 临时文件 + rename（`host-core::atomic_write::write_secret`）。
- 读取：文件缺失返回默认空配置/状态；JSON 非法回 `Error`（不静默覆盖）。
- Bot 密钥：`saveBot` 只在配置中写 `credentialRef`/`webhookSecretRef`，不把明文写入配置文件；
  当前未接 Host credential 服务，密钥值不落库（见 §5-1）。

## 3. 接口

`bots` channel（服务级参数由 `ProxyChannel` 打包为数组）：

```ts
getConfig(): BotsConfigFile
saveConfig(config): BotsConfigFile
listBots(): BotConfig[]
saveBot({bot, credentialValue?, webhookSecretValue?}): BotConfig
removeBotSecret(botId): BotConfig
deleteBot(botId): void
getStatus(): BotStatus
getBotStates(): BotContextState[]
resetBotState(contextKey): void
createBindCode({botId?, allowedWorkspaces?, ttlMs?}): {code, expiresAt}
listWorkspaceRefs({currentWorkspace?}): BotWorkspaceRef[]
syncAppRuntimePreferences(prefs): void
```

错误：普通 `Error`；未实现成员由 Gateway 回显式 `NotImplemented`。

## 4. 远端 workspace

- `listWorkspaceRefs` 的 `id = workspaceIdentity?.trim() || workspacePath`；`label = basename(workspacePath)`。
- 远端条目必须保留 `workspaceIdentity`；后续群任务/附件迁移时继续传递该字段，不得仅按路径匹配。
- `BotConfig.allowedWorkspaces` 存 identity 值（与 TS `normalizeAllowedWorkspaces` 同口径）。

## 5. 已登记差异

| #   | 差异                                                                          | 原因                                                                      | 移除条件                                  |
| --- | ----------------------------------------------------------------------------- | ------------------------------------------------------------------------- | ----------------------------------------- |
| 1   | `credentialValue`/`webhookSecretValue` 不写入 Host credential store，只写引用 | bots channel 未接 `CredentialService`；平台运行时也未迁移，暂不影响本子集 | 接 Host credential 服务后补齐 save/delete |
| 2   | `saveBot` 不调用 provider `resolveName`、不做 BotConfig 全量归一化/校验       | 同上，provider 未迁移                                                     | 迁移 provider 与共享 schema 校验后补齐    |
| 3   | `getConfig` 缺文件时返回默认值但不写回文件                                    | 避免只读调用产生写副作用；TS 会写回默认文件                               | 需要与 TS 文件字节对拍时改为写回          |
| 4   | 读改写使用进程内 Mutex，不是跨进程文件锁                                      | Host 是唯一写者；迁移期双 Host 不承诺并发写                               | 双 Host 共存期需要时改用 `file_lock`      |
| 5   | `createBindCode` 只生成/保存在内存，绑定消费未实现                            | 入站消息链路未迁移                                                        | 迁移平台 runtime 后补消费与过期清理       |
| 6   | `getStatus.botRuntime` 只给 idle/disabled，不含真实 provider 连接状态         | 平台 runtime 未迁移                                                       | 迁移 provider 后补                        |

## 6. 未迁移的 20 个成员（阻塞项）

`beginFeishuRegistration`/`pollFeishuRegistration`/`beginWeixinRegistration`/`pollWeixinRegistration`、
`handleInboundMessage`/`handleProviderCallback`/`handleProviderCallbackResponse`、
`getGroupMemberNames`/`setGroupEnabled`/`getTopicDetails`/`readTopicResource`/`validateTopicResource`、
`resendGroupResult`/`reconcileGroupResult`/`retryPrivateDelivery`/`retryTopicPreparation`、
`getUserConfigOptions`/`testBot`/`watchAutomationRun`/`replyToChannel`。

这些成员依赖：

1. 平台 provider（Telegram Bot API、飞书/Lark WebSocket/Card、WeChat 轮询、Webhook）；
2. Bot conversation state 的群/话题/投递状态机与任务服务；
3. 附件与 V4 流（`bots-v4-stream.md`）；
4. 远端 workspace runtime（Bot 私聊/群任务）。

## 7. 验收

- RPC e2e：`rpc_bots.rs` 覆盖 12 个成员（配置/状态/绑定/引用/缺口）。
- 单测：待补 JSON schema 归一化与并发写。
- 真实客户端/TS 差分：配置/状态文件与 TS 的写出逐字节对拍（同字段、同缩进）待 `saveBot` 归一化补齐后执行。
