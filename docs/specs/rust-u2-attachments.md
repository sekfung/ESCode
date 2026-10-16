# Rust Host `prompt-attachment-transfer` 与 `media-preview`（U2）

> 状态：交付 · 2026-10-15。TS 真相源：`packages/services/src/prompt-attachment-transfer/promptAttachmentTransferService.ts`（本地实现）、
> `packages/services/src/media-preview/mediaPreview.ts`。

## 1. prompt-attachment-transfer（5 个成员，本地实现）

本地 workspace 保持 `localPath` 零拷贝，不伪造上传进度：

| 成员                       | 行为                                                                                                                               |
| -------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| `stage(params)`            | `{operationId, ref: localPath, bytes, staged:false}`；`bytes` = `sizeBytes`（数值且 >0）否则 `stat(localPath).size`，stat 失败为 0 |
| `adopt / cancel / cleanup` | 无操作，回 `undefined`                                                                                                             |
| `onDynamicProgress(opId)`  | 事件流，本地实现从不发进度（订阅有效、永不触发）                                                                                   |

远程 host 的跨机暂存属于 remote 域（SSH/WSL/Docker），不在本片。

## 2. media-preview（`prepare`；`refreshPlaybackUrl`/`release` 待做）

`prepare({path, expectedKind})`（audio/video）：

1. `getMediaPreviewFormat(path)`：路径 `\`→`/` 并小写后做 `endsWith` 匹配共享格式表；表外或 `kind` 与 `expectedKind` 不符 → `Unsupported media preview format: <path>`。
2. `file.stat`：非文件 → `Path is not a media file: <path>`（不存在沿用 `file.stat` 的 Node 风格错误）。
3. 内联路径（Rust Host 没有本地媒体 HTTP 服务）：`size > 8 MiB` → `Media file is too large for inline preview: <path>`；否则复用 `file.readMediaPreview`，
   回 `{kind:"inline", dataBase64, mediaType, path, size}`。

## 3. 登记差异（不修）

1. **`local-url` 分支见 §5**（Main 授权 + Host 构造 URL）；Main 未声明该能力（旧 Main）时，Rust Host 仍走内联（≤8 MiB）。`host-range-url` 由桌面 Main 侧的 `remoteMediaPreviewProxy.ts`（远程 SSH 作用域的本地 HTTP 代理）产生，不属于 Host 服务——这不是 Host 缺口，而是 Main 侧职责。
2. `refreshPlaybackUrl` / `release` 同属远程 SSH 作用域的 TS 代理（`desktop/src/host/remoteMediaPreviewProxy.ts`，只覆盖远程 workspace 的 `IMediaPreviewService`）；
   本地 `createMediaPreviewService` 没有这两个方法，Rust Host（本地 workspace）不实现——这不是缺口。

## 4. 验收

- `rustHostAttachments.test.ts`：对真实 TS `createLocalPromptAttachmentTransferService` / `createMediaPreviewService`（内联模式）与 Rust Host 逐项 `toEqual`
  （`sizeBytes` 命中/缺省/0、文件缺失；mp3/mp4/大写扩展名、格式不符、目录、缺失文件、超限、恰好 8 MiB 边界）；事件订阅有效且无泄漏。
- 负向验证：改 `bytes` 取值顺序、改超限判定，对应用例失败。

## 5. media-preview `local-url`（本地）

**所有者**：授权注册表 = Main（`localMediaPreviewPathRegistry`：规范化路径并记录已授权）；`escode-media://` 协议的实际文件服务 = Main（`localMediaPreviewProtocol.ts`，
按 `isPathAuthorized` 放行）。Host 只做：校验格式/stat → 请求 Main 授权 → 用纯函数构造 URL。Host 不保存授权状态。

```
Renderer ─prepare→ Rust Host ──localMediaPreviewAuthorize{requestId,path}──→ Main(registry.authorize)
                       ↑                                                          │
                       └──────localMediaPreviewAuthorized{requestId,ok,path|error}┘
Rust Host → {kind:"local-url", path:<canonical>, url:escode-media://local/preview?path=<form-urlencoded>}
Renderer ─GET url→ Main protocol handler（isPathAuthorized 才放行）
```

**协议（控制通道，host-control `ControlMessage` 新增两个变体）**：

| 方向      | kind                          | 字段                                       |
| --------- | ----------------------------- | ------------------------------------------ |
| Host→Main | `localMediaPreviewAuthorize`  | `requestId: string`、`path: string`        |
| Main→Host | `localMediaPreviewAuthorized` | `requestId`、`ok: bool`、`path?`、`error?` |

`rpcListen` 新增可选 `localMediaPreview: bool`（缺省 false）：Main 声明自己会应答授权请求；未声明则不建授权 broker，`prepare` 走内联。

**语义（与 TS `createMediaPreviewService` 同）**：

1. 顺序：格式 → `stat`（非文件报错）→ 授权 → 回 `local-url`；**有授权能力时不做 8 MiB 内联限制**（大视频的意义所在）。
2. `local-url` 的 `path` 是授权后的规范路径，`size` 是 `stat.size`，`mediaType` 来自格式表。
3. URL 与 TS `buildLocalMediaPreviewUrl` 逐字相同：`escode-media://local/preview?path=` + `application/x-www-form-urlencoded`（空格 → `+`，保留 `*-._` 与字母数字）。
4. Main 授权失败（`ok:false`）→ `prepare` 以 `error` 原文失败（TS 同：`authorize` 抛错即 `prepare` 抛错）。
5. 无超时（TS 也没有；Main 退出时控制通道 EOF，Host 随之收口，挂起的请求随进程消亡）。`requestId` 不匹配的应答丢弃（stale 防护）；控制通道已关闭 → `prepare` 报 `Control channel closed`。
6. 并发：每个请求独立 `requestId`，pending 表是 broker 的唯一状态；应答只消费一次。

**验收**：

- Rust：`media_preview_tests`（broker 单测：应答配对、乱序、未知 `requestId` 丢弃、`ok:false`、通道关闭；URL 向量与 TS 对账）；`control_tests`（新变体的 serde 往返、`rpcListen.localMediaPreview` 缺省）；host-proto 契约清单含两个新 kind。
- TS：`rustHostAttachments.test.ts` 增加 local-url 场景——桩 Main 应答授权，与真实 `createMediaPreviewService({authorizeLocalMediaPreviewPath, createLocalMediaPreviewUrl})` 逐项 `toEqual`
  （含空格/中文/`#`/`?`/`%` 路径、超过 8 MiB 的大文件、授权拒绝）；Main 侧 `rustHostBridge` 单测：收到 `localMediaPreviewAuthorize` → 调用注册表 → 回 `localMediaPreviewAuthorized`（成功与抛错）。
- 负向验证：URL 编码改成 `encodeURIComponent`、去掉授权调用、乱序应答配错，对应用例失败。

**§5 交付记录（2026-10-16）**：`host-control/path_authorizer.rs`（pending 表 broker，收口时 `close()` 释放写句柄）、`ControlMessage` 两个新变体 + `rpcListen.localMediaPreview`、
`domains_attachments::prepare` 异步化并接授权、`main.rs` 控制循环配对应答、Main 侧 `rustHostBridge.answerAuthorize`（注册表为 `localMediaPreviewPathRegistry.authorize`）。
验收：Rust 405 项全绿（含 broker 5 项、prepare 6 项、URL 向量由 Node `URLSearchParams` 生成并对账）；`rustHostAttachments.test.ts` 的 local-url 场景与真实 TS 服务逐项相等
（空格/中文/`#`/`?` 路径、规范化改写、超内联上限的大文件、授权拒绝、校验先于授权）；`rustHostRuntimeEnv.test.ts` 的 Main 侧两项（声明能力并按请求应答成功/拒绝；无授权器不声明）。
负向验证：让 Rust 跳过授权 → local-url 差分失败（已恢复）。
**登记差异**：授权请求无超时（与 TS 同）；Host 在控制通道关闭后挂起的 `prepare` 以 `Control channel closed` 失败。
