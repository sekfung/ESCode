# circuit-protocol

Circuit IR v1、识别草稿与 Circuit MCP 工具的线协议（docs/circuit/README.md §4、§5）。
纯 domain：只有常量与 zod schema，零 IO、零状态。IR 的唯一所有者是 Host 内的 `CircuitService`（ADR 0005），
本模块只被服务、插件、Agent 运行时、远程访问层和面板读取。

## 边界规则

- 所有请求、响应、事件都必须是严格的：对象一律 `.strict()`，未知字段直接拒绝。
- `confidence` 只能由服务端写入。schema 强制 `parser = 1`、`recognition < 1`，`correction` 必须带 `author`；
  `correction` 的具体取值由服务端决定（未在规格中定义，见「待定」）。
- 识别草稿（`circuitRecognitionDraftSchema`）不包含 `confidence` 字段；带 `confidence` 的输入会被拒绝。
  草稿里的元素 id 只用于草稿内部互相引用，提交后由服务端生成最终 id 并改写 `pinRefs` / `componentIds`。
- 引用完整性（`pinRefs.componentId`、`pin.netId` 是否指向真实元素）不在 schema 里校验：
  服务端校验后写入 `diagnostics` 并降低相应元素的置信度（§4），schema 保持结构性校验。
- 源文件路径与固件路径都是 workspace 相对路径；绝对路径、盘符、UNC、`..` 段在 schema 层拒绝。
- `contentHash` 是 sha256 十六进制；图片和 PDF 以内容哈希区分版本（§3.2）。
- 分页游标 `cursor` / `nextCursor` 是不透明字符串，内容由服务端定义；缺失 `nextCursor` 表示没有下一页。
  单次响应上限 `CIRCUIT_RESPONSE_MAX_BYTES`（64 KiB）。
- 错误码固定为 `unavailable | not_found | unsupported_format | invalid_ir | stale_revision | too_large | io_error`。
- 路径与格式判定、批准拦截（`circuit_export`）都不在本模块：批准由插件层负责（§5）。

## 待定（需要时按规格补充，不要在本模块自造兜底）

- `correction` 元素的 `confidence` 取值规则（服务端决定）。
- `circuit_submit_recognition` 对同一 `contentHash` 重复提交的语义（服务端决定）。
- `circuit_get` 附带 `diagnostics` 明细、`circuitReferenceSchema` 的 `componentCount` / `netCount`
  是为满足 §3.5 降级卡片与「校验问题可见」补的字段，规格只写了摘要要点。
- `circuit_export` 的目标文件扩展名（§8 提到 `.circuit.json`）由服务端约束。
