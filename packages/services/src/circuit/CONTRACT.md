# circuit-service

Circuit IR 的唯一所有者（ADR 0005、docs/circuit/README.md §3）。Host 侧服务：
确定性解析（KiCad / 网表 / BOM）、校验、修正、revision、持久化、源文件监听都在这里；
Agent 经 broker、面板经 hooks 访问它，两者都不直接读写 IR 文件。

## 边界

- `contract.ts` 是唯一的公开契约（`ICircuitService` + 频道 `circuit`）；`index.ts` 是 browser-safe 入口，
  `node.ts` 是 Host 装配入口。domain / app / adapters 三段：domain 只做确定性解析与 IR 校验（零 IO），
  app 持有状态与事件顺序，adapters 负责应用数据目录、workspace 读取与监听。
- 写入只有一个方法（`correct`）且必须带 `baseRevision`：版本落后抛 `stale_revision`，不覆盖、不静默合并（§3.3）。
- 事件只有一个（`onChanged`）：`{ irId, revision, stale }`。写入事件在**持久化完成之后**发出；
  源文件变化只把 `stale` 翻成 true、`revision` 不变（§3.4）。面板按 `(revision, stale)` 与本地状态比对决定刷新，
  §3.3 的「忽略 revision ≤ 本地」只适用于 revision 前进的刷新，stale 翻转由 `stale` 字段触发。
- 同一 `(identityKey, sourcePath)` 的写入串行执行；事件因此按 revision 单调递增。
- `query` 的 `ref` / `netId` / `netName` / `blockId` / `blockName` 都是可选过滤：省略即列出该类全部元素
  （面板的 BOM 与网络表走这条只读路径，不另开契约，也不从 layout 反推 IR 事实）；
  id 与名字同时给出无法判断意图，schema 直接拒绝。结果同样按响应上限分页。
- 只读分析：`compareFirmware`（§7）不写状态、不需要 `baseRevision`，结果按响应上限（§5）分页，
  游标实现与 `query` 共用一份（`app/circuitCursor.ts` + `app/analysisPagination.ts`），
  签名绑定各自的查询参数，拿错游标报 `invalid_ir`。
- `compareFirmware` 先按扩展名判格式（`.dts` / `.dtsi` / `.overlay` / `.ioc` / `.h`，其余报
  `unsupported_format`）再读文件；IR 里没有可识别的 MCU 引脚名时报 `invalid_ir`，不做静默降级。
- 元素 id：根图纸元素是 KiCad UUID，子图纸元素带实例路径前缀（`/<sheetUuid>/<uuid>`）；
  网表 / BOM 没有 UUID，用 `netlist:<位号>`、`bom:<位号>`。网络 id 由名字（global/电源/hierarchy/local 标签）
  或首个引脚锚点派生；同名 local / hierarchy 标签只在同一图纸实例内成网，所以这类网络 id 带作用域后缀。
  这一切都是为了让「重新解析后按 id 重新套用修正」成立。
- 修正记录（corrections.json）保留作者、会话 id、时间、目标元素与修正内容；重新解析后套用不上的记录
  保留在磁盘并标 `orphaned`，面板列出后由用户确认或删除（只能删除 orphaned 的记录）。
- `CircuitLayout` 是渲染几何的只读派生视图：与 IR 同一次解析产出、按 revision 对齐、不进 IR、不参与修正（§8）。
- 导出走 `exportSnapshot`（契约方法，实现见 `app/circuitExport.ts`）：写入 `snapshot` 端口的 workspace 派生产物，
  内容带 `derived: true` 标记；路径必须落在 workspace 内且以 `.circuit.json` 结尾，否则抛 `unsupported_format`。
  导出**不写 store、不发事件、不注册监听**，服务永远不读回这份快照（§2 规则 8）。
- Agent 链路（§5）在 `app/circuitAgentBridge.ts`（op → 服务方法、错误码 → 失败信封）与
  `app/circuitAgentRequestRouter.ts`（`interaction/circuit*` 严格校验与应答），覆盖已注册的六个 op
  （parse/get/query/correct 与只读的 compareFirmware、export）；
  `app/circuitWriteQueue.ts` 持有「同一 IR 写入串行」队列。桥接层不持有状态、不缓存 IR。
- 日志用 `createServiceLogger("circuit")`，只记 id、计数、耗时，不写电路内容与文件内容（§9）。

## P1 已知边界（按规格声明，不另加兜底）

- 总线成员不展开：`(bus)` 只记入 `limitations`，总线分支上的标签照常成网。
- 子图纸最多 64 个文件 / 8 层；层级成环与未加载的子图纸记入 `limitations`，不猜测内容。
- `.kicad_pcb` 的 layout 只给器件落点与板框尺寸，不画铜箔与走线。
- 同一个 workspace 被多个窗口 Host 同时打开时，revision 检查是各自服务内的乐观并发，不做跨进程锁。
- 固件比对：只解析 §7.1 列出的构造。devicetree 的 `pinctrl-0 = <&label>` 间接引用与 `NRF_PSEL(...)`
  跳过；字母端口按 `P<口><脚>` 比对，数字端口控制器（`&gpio1 13`）按 `P<口>.<脚>` 与 `GPIO<脚>` 两种
  候选匹配 IR 引脚名（同一控制器的两种写法用信号名消歧）；`.ioc` 的通用 GPIO 无标签时跳过。
  固件内部冲突与引脚声明冲突都按「排序在后的那条声明」给出（码点序，保证跨平台一致），
  因此「原理图独有」条目的 `file` 是原理图源文件、`line` 省略。

## 待定（需要时按规格补充）

- `correction` 元素的 `confidence` 取值：目前人工修正记 1、Agent 修正保留原置信度（新建元素取 0.5）。
- 网络 id 的稳定性依赖「名字或首个引脚锚点不变」；结构性改动后引用旧 id 的修正会变 orphaned（符合 §3.4）。
