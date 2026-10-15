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

1. **无 `local-url` 分支**：TS 桌面 Host 由 Main 注入 `authorizeLocalMediaPreviewPath` 与本地媒体 URL 构造，返回 `{kind:"local-url", url}`；
   Rust Host 没有该 HTTP 服务，永远走内联（≤8 MiB）。大视频预览需等 Host 内置媒体 range 服务（`host-range-url` 形态）后再补。
2. `refreshPlaybackUrl` / `release` 仅 `host-range-url` 形态用到，随媒体 range 服务一起做。

## 4. 验收

- `rustHostAttachments.test.ts`：对真实 TS `createLocalPromptAttachmentTransferService` / `createMediaPreviewService`（内联模式）与 Rust Host 逐项 `toEqual`
  （`sizeBytes` 命中/缺省/0、文件缺失；mp3/mp4/大写扩展名、格式不符、目录、缺失文件、超限、恰好 8 MiB 边界）；事件订阅有效且无泄漏。
- 负向验证：改 `bytes` 取值顺序、改超限判定，对应用例失败。
