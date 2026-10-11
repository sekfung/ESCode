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
- `circuit_query` 的 `ref` / `netId` / `netName` / `blockId` / `blockName` 都是可选过滤：全部省略即列出
  该类全部元素（同样分页），面板的 BOM 与网络表走这条只读路径；`netId` 与 `netName`（或 `blockId` 与
  `blockName`）同时给出时无法判断意图，schema 拒绝。`circuit_get` 仍只返回摘要，不承担元素列表。
- 错误码固定为 `unavailable | not_found | unsupported_format | invalid_ir | stale_revision | too_large | io_error`。
- 路径与格式判定、批准拦截（`circuit_export`）都不在本模块：批准由插件层负责（§5）。
- Agent 链路（`agentTools.ts`）：broker 信封 + 反向方法 `interaction/circuit*` 与 serial 同构；
  工具参数/响应直接复用本模块的工具总表，协议层不另造字段。当前只列已注册的七个 op
  （parse/get/query/correct/check/compareFirmware/export），`circuit_submit_recognition`（G6）
  合入时再补，不预留空实现。
- 连接材料 env（`ESCODE_CIRCUIT_BROKER_SOCKET` / `ESCODE_CIRCUIT_BROKER_TOKEN`）只定向注入 circuit server；
  Host 能力标记 `ESCODE_HOST_CIRCUIT` 由 Agent 运行时用来决定是否注册 circuit MCP server。
- 工具失败走响应信封 `{ ok: false, error: { code, message } }`（不是 JSON-RPC error）：
  `CircuitServiceError.code` 原样成为 `error.code`，broker 与插件只透传、不改写。

## 待定（需要时按规格补充，不要在本模块自造兜底）

- `correction` 元素的 `confidence` 取值规则（服务端决定）。
- `circuit_submit_recognition` 对同一 `contentHash` 重复提交的语义（服务端决定）。
- `circuit_get` 附带 `diagnostics` 明细、`circuitReferenceSchema` 的 `componentCount` / `netCount`
  是为满足 §3.5 降级卡片与「校验问题可见」补的字段，规格只写了摘要要点。
- `circuit_check` 与 `circuit_compare_firmware` 的请求都带可选 `cursor`：规格 §5 表格只列了
  `irId` / `files`，但同一条「超出部分通过 cursor 分页获取」对全部工具成立，服务端两个 op
  都按响应上限分页，因此请求 schema 显式带上它（缺失 `nextCursor` 表示没有下一页）。
- `circuit_export` 的目标文件扩展名由服务端约束为 `.circuit.json`（导出 `CIRCUIT_EXPORT_SUFFIX` 供插件描述复用）。
