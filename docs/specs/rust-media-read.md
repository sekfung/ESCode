# Rust 媒体 Read（图片、PDF、视频）与工具结果媒体

2026-09-26。用户选定的功能缺口（rust-release-rollback.md「功能缺口范围」中的「MCP OAuth + 媒体 Read」）。

对齐的 TS 实现：

| 能力             | TS 文件                                                          |
| ---------------- | ---------------------------------------------------------------- |
| Read 媒体分支    | `core/src/tool/handlers/read{,-image,-pdf,-video}.ts`            |
| 图片预算压缩     | `adapters/src/image/*`（Jimp）                                   |
| PDF 页渲染       | `adapters/src/pdf`（Poppler）                                    |
| 工具结果媒体投影 | `adapters/src/model/{transform,tool-result-media-projection}.ts` |

## 分期

| 期          | 内容                                                                                                | 验收                                                                                                         |
| ----------- | --------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------ |
| 1（已完成） | 图片 Read 与预算压缩，工具结果媒体的会话持久化，以及三种协议的投影                                  | 三种协议下 Node/Rust 请求中的工具结果与媒体消息形态一致；预算内原图字节一致；超尺寸图两侧均为 2000 边长 JPEG |
| 2（已完成） | PDF：原生（模型支持 PDF 时，≤20MB、≤10 页）与 `pages` 页渲染；Read schema 与描述随模型 PDF 能力变化 | 同上，外加 schema 差分                                                                                       |
| 3（已完成） | 视频 Read（base64 直传、大小上限）                                                                  | 同上                                                                                                         |
| 4           | 附件大图缩放                                                                                        | 附件请求形态差分                                                                                             |
| 5           | MCP 工具结果中的图片走同一媒体通道                                                                  | MCP 差分                                                                                                     |

## 规则（第 1 期）

- 扩展名分派顺序与 TS 相同：图片（`.jpg/.jpeg/.png/.gif/.webp`）→ 视频 → PDF（模型支持时）→ 文本。
- 图片读取上限 20MB。
- 预算：
  - 原始字节不超过 3.75MB；
  - base64 不超过 5MB；
  - `ceil(base64 字节 × 0.125)` 不超过 25000 token。
- 以文件头识别格式：
  - WebP 只做直传，超预算报 `unsupported`；
  - 其余格式：尺寸 ≤2000 且满足预算时原样返回，策略记为 `original`。
- 压缩级联按 TS `findFirstFittingCandidate` 的顺序：
  1. 保格式（PNG 只在原尺寸尝试一次最优压缩）；
  2. 缩到 2000；
  3. JPEG 质量依次 80/60/40/20；
  4. 按 0.75/0.5/0.25 逐级缩放；
  5. 激进 JPEG q20，最长边依次 1000…200。
- 缩放尺寸与 Jimp `scaleToFit` 相同：`round(w·f)`、`round(h·f)`，结果为 0 时取 1。
- 已知差异：Rust 编码器与 Jimp 输出的字节不同。因此压缩后的字节不比对，边界附近选中的候选也可能不同；比对的是策略顺序、尺寸与格式。预算内原图字节一致。
- 模型可见内容：只有一个 image 块，不附尺寸说明（TS `formatReadImageOutput`）。
- 持久化：
  - 媒体字节写入附件存储，会话消息只保留 `_zcode_attachment` 引用，与用户附件同一机制，base64 不进入会话库；
  - 本次运行内的后续请求直接使用内存中的数据。
- 投影（TS `transform.ts`）：
  - Chat Completions：
    - tool 消息只放文本化结果（图片为 `[Attached image/png: Read image]` 类占位）；
    - 同一步的所有工具结果之后，追加一条 user 消息：`Tool result media from Read:` 加图片 part。
  - Anthropic Messages：`tool_result` 内嵌 image 块。
  - OpenAI Responses：`function_call_output` 结构化输出含 `input_image`。
  - 工具报错时只给文本。

- Anthropic 请求体顺带对齐两处与媒体无关的差异（由本差分发现）：
  - `tool_result` 只在错误时写 `is_error`（AI SDK 行为）；
  - 主请求把最新的非 system 消息设为缓存断点（TS `finalizeLatestNonSystemMessageCacheControl`），`cache_control` 落在其最后一个内容块上；
    - 投影阶段合成的「媒体后置」user 消息不计入，与 TS 在投影前标记相同。
- Responses 工具定义不带 `strict` 字段，与 AI SDK 输出一致；此前 Rust 固定写 `strict:false`。
- 上下文估算：本轮内联媒体 part 与附件同口径计入，图片计 3072 字符，其余取实际字节与 64KiB 的较小值；base64 长度不计入。
- ReadSessionContext 呈现媒体工具结果时，按占位文本（`[Attached <mime>: <name>]`）输出。

## 规则（第 2、3 期）

- 模型能力：每轮开始时，工具侧按本轮模型的 `inputFormat` 调整。
  - 支持 PDF 时，Read 的 schema（增加 `pages`）与描述（在图片行后插入 PDF 行）取自 TS `resolveReadInputSchema` 与 `resolveReadProviderDescription` 的生成资产；
  - 同时记录该会话的 inputFormat，供 Read 执行分支使用。
- PDF 只在模型支持 PDF 时走专用分支，否则按文本读取，与 TS 相同。
  - 不带 `pages`（原生）：
    - 上限 20MB；
    - pdfinfo 可用时超过 10 页即失败，并提示使用 `pages`；
    - 校验 `%PDF-` 文件头；
    - 模型收到 `PDF file read: <path> (<size>)` 文本与 file 块。
  - 带 `pages`：
    - 模型不支持图片时失败；
    - 上限 100MB；
    - 页范围校验与 TS `getReadPdfPagesValidationFailure` 相同（每次最多 20 页）；
    - 先探测 `pdftoppm -v`（缺失时给出安装提示），再以 `pdftoppm -jpeg -r 100 -f -l` 渲染到临时目录；
    - 每页按图片预算压缩，模型收到 `PDF pages extracted: …` 文本与各页 image 块（名称 `PDF page N`）；
    - 失败分类与文案对齐 Poppler 适配器：密码、页码越界、损坏、I/O、超时。
- 视频：
  - 扩展名为 `.mp4/.m4v/.mov/.webm/.mkv/.avi`，上限 30MB，不转码，空文件失败；
  - 含视频的工具结果在所有协议上都文本化，媒体后置为 user part，与 TS `toolResultHasVideoMedia` 相同。

## 所有者

- tools 负责读取与压缩，并在 `ToolOutput` 上携带媒体块。
- 会话 owner 在工具结果提交时，把媒体写入附件存储并替换为引用。
- model 协议层负责按 API 格式投影；读取历史消息时解析附件引用。
