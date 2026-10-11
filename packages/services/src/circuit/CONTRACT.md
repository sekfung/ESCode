# circuit-service

Circuit IR 的唯一所有者（ADR 0005、docs/circuit/README.md §3）。Host 侧服务：
确定性解析（KiCad / 网表 / BOM）、校验、修正、revision、持久化、源文件监听都在这里；
Agent 经 broker、面板经 hooks 访问它，两者都不直接读写 IR 文件。

## 边界

- `contract.ts` 是唯一的公开契约（`ICircuitService` + 频道 `circuit`）；`index.ts` 是 browser-safe 入口，
  `node.ts` 是 Host 装配入口。domain / app / adapters 三段：domain 只做确定性解析与 IR 规则（零 IO），
  app 持有状态与事件顺序，adapters 负责应用数据目录、workspace 读取与监听。
- 写入只有一个方法（`correct`）且必须带 `baseRevision`：版本落后抛 `stale_revision`，不覆盖、不静默合并（§3.3）。
- 事件只有一个（`onChanged`）：`{ irId, revision, stale }`。写入事件在**持久化完成之后**发出；
  源文件变化只把 `stale` 翻成 true、`revision` 不变（§3.4）。面板按 `(revision, stale)` 与本地状态比对决定刷新，
  §3.3 的「忽略 revision ≤ 本地」只适用于 revision 前进的刷新，stale 翻转由 `stale` 字段触发。
- 同一 `(identityKey, sourcePath)` 的写入串行执行；事件因此按 revision 单调递增。
- `query` 的 `ref` / `netId` / `netName` / `blockId` / `blockName` 都是可选过滤：省略即列出该类全部元素
  （面板的 BOM 与网络表走这条只读路径，不另开契约，也不从 layout 反推 IR 事实）；
  id 与名字同时给出无法判断意图，schema 直接拒绝。结果同样按响应上限分页。
- 只读分析：`check`（§6）与 `compareFirmware`（§7）都不写状态、不需要 `baseRevision`。
  两者的结果都按响应上限（§5）分页，游标实现与 `query` 共用一份（`app/circuitCursor.ts` +
  `app/analysisPagination.ts`），签名绑定各自的查询参数，拿错游标报 `invalid_ir`。
- `check` 读 workspace 下 `.escode/circuit-rules.json`（只读）：没有 → `config.status="default"`；
  合法 → `"applied"`；解析失败 / 结构不符 → **整份忽略**并返回 `"invalid"` + `reason`（按默认配置检查）。
  条目涉及的器件或网络低置信度时 `lowConfidence=true`。
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
  `app/circuitAgentRequestRouter.ts`（`interaction/circuit*` 严格校验与应答），覆盖已注册的八个 op
  （parse/submitRecognition/get/query/correct 与只读的 check/compareFirmware、export）；
  `app/circuitWriteQueue.ts` 持有「同一 IR 写入串行」队列。桥接层不持有状态、不缓存 IR。
- 日志用 `createServiceLogger("circuit")`，只记 id、计数、耗时，不写电路内容与文件内容（§9）。

## 识别提交（§5 `circuit_submit_recognition`、§2 规则 2/3/4、§3.2）

`submitRecognition` 与 `parse` 共用写入队列、store、源文件监听与事件顺序（持久化之后才发 `circuit.changed`），
编排在 `app/circuitRecognition.ts`，纯逻辑在 `domain/recognition*.ts`（零 IO）：

- **来源**：扩展名先判后读。`.pdf` 用仓库已有的 pdfjs-dist（`adapters/pdfVector.ts`）提取文本项与线段；
  页内只有位图（提取结果为空）时按 `image` 处理。图片扩展名为 png/jpg/jpeg/webp/bmp/gif；
  其余（如 `.kicad_sch`）抛 `unsupported_format`——确定性解析走 `circuit_parse`。读字节与内容哈希走
  `CircuitSourcePort`（`readBytes` / `hashBytes`），矢量证据走 `CircuitVectorPort`。
- **草稿校验与 id**：草稿（`circuitRecognitionDraftSchema`）不带 confidence；最终 id 由服务端生成并同步改写
  `pinRefs` / `componentIds` / `pins[].netId`——器件是 `recognition:<位号>`（重复位号加 `#n`），
  网络与模块复用确定性解析的命名规则（`makeNetId` / `makeNamedElementId`，同名同引脚集的重复网络加 `#n`）。
  指向不存在草稿 id 的引用**原样保留**，由 `validateCircuitIr` 报 `unknown-component-reference` /
  `unknown-net-reference`（不静默丢弃，也不新建占位元素）。
- **置信度**（只能由服务端给出，`domain/recognitionConfidence.ts`）：基准 0.9，按元素涉及的诊断类别扣分——
  单引脚网络 −0.1、引脚数与封装不一致 −0.2、位号重复 −0.3、引用不存在的器件/网络 −0.3；同一类只扣一次、
  多类累加，结果夹在 [0.05, 0.9]，**永不等于 1**。`dangling-net`（零引脚网络）只写诊断、不扣分。
- **矢量 PDF 证据**（§2 规则 3，`domain/recognitionEvidence.ts`）：器件的位号、网络的名称在元素 `region`
  附近（容差 = max(2, 0.25 × region 长边)，页码匹配）出现为 PDF 文本项时，该元素 `origin.by = "parser"`、
  `confidence = 1`，**优先于扣分**（诊断仍保留在 IR 里）。只做精确串匹配（不区分大小写）；
  提取到的线段用于判断「这份 PDF 是矢量的」与后续几何使用，不单独给元素提置信度。
  `region` 坐标口径：渲染后左上角为原点（图片=像素，PDF=点），与面板叠框一致。
- **「引脚数与封装一致」的口径**（`domain/recognitionFootprint.ts`）：只在该器件的 `footprint` 能解析出
  封装编号时才比对——白名单封装族（SOIC/TSSOP/QFN/LQFP/DIP… 的 `<族>-<引脚数>`）与
  `PinHeader_1x04` / `PinSocket_2x05`（m × n）。解析不出来就**不判定**（不写诊断、不扣分）：
  IR v1 只带封装名字符串，仓库里没有封装库可以查期望引脚数，这是已声明边界。
- **IR 标识与前置条件**（§3.2/§3.4）：`irId` 由 (身份 key, 源路径, **内容哈希**) 派生，同一份内容永远得到同一个
  irId；内容一变就是一份新 IR（`revision` 从 1 重新起算，旧修正不迁移，只记进日志）。
  同一份内容的 IR 已存在时沿用 `parse` 的前置条件（`requireReparseRevision`）：没有 `baseRevision` 抛
  `invalid_ir`、落后抛 `stale_revision`、一致则 `revision + 1` 并按 id 重新套用已有修正（套不上的标 orphaned）。
  `source.kind` 取 `pdf_vector` / `image`；识别结果没有渲染几何（`layout` 为空），面板在原图/PDF 上叠识别框。

## 规则判定口径（§6、§6.2）

规则只读 Circuit IR（§2 规则 1），IR v1 不带引脚电气类型（`power_in` / `no_connect`）、不带 `.kicad_sch`
的 no-connect 标记、也只留同一网络的一个标签名。下面的代理口径是确定性的（domain/circuitRules\*.ts）：

| ruleId               | 判定                                                                                                                                                                                                                                                                                         |
| -------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `unconnected-pin`    | 引脚没有 `netId`，或所属的**未命名**单引脚网络（解析器的 `Net-(…)` 约定 / 识别结果没给名字）只有它自己。跳过引脚号为空（机械孔）与引脚名形如 `NC` / `N.C.` / `DNC` 的引脚                                                                                                                    |
| `single-pin-net`     | `net.pinRefs.length === 1`（零引脚网络由 `dangling-net` 诊断负责）                                                                                                                                                                                                                           |
| `undriven-power`     | 网络上有电源输入引脚（引脚名匹配 `VCC`/`VDD`/`VBAT`/`VIN`/`+3V3`/`1V8`…，不含 GND/VSS 族），且网上没有任何驱动源：连接器/电池类位号（`J`/`P`/`CN`/`USB`/`PS`/`BT`/`BAT`）的任一引脚，或无源件（`C`/`L`/`FB`/`F`/`D`/`Y`/`X`/`R`/`RN`/`TP`/`JP`）之外器件的非电源输入引脚（如 LDO 的 `VOUT`） |
| `i2c-missing-pullup` | 网名以 `SDA` / `SCL` 结尾（`SCLK`、`SDIO` 不算）的网络里没有电阻（`R` / `RN`）的另一端连到非地电源轨                                                                                                                                                                                         |
| `missing-decoupling` | IC（`U` / `IC` 前缀）的电源输入引脚所在网络上没有电容（`C` 前缀）落在 GND/VSS 族网络；IR 无坐标，「附近」用同一网络代替                                                                                                                                                                      |
| `net-name-conflict`  | 同名但 id 不同的多个网络（例如同一子图纸实例化两次时各自成网的 local 标签）。「同一网络有多个名称」在 IR v1 里不可见，属已声明边界                                                                                                                                                           |

## P1 已知边界（按规格声明，不另加兜底）

- 总线成员不展开：`(bus)` 只记入 `limitations`，总线分支上的标签照常成网。
- 子图纸最多 64 个文件 / 8 层；层级成环与未加载的子图纸记入 `limitations`，不猜测内容。
- `.kicad_pcb` 的 layout 只给器件落点与板框尺寸，不画铜箔与走线。
- PDF / 图片不在这里解析（属于 `circuit_submit_recognition`，G6）。
- 同一个 workspace 被多个窗口 Host 同时打开时，revision 检查是各自服务内的乐观并发，不做跨进程锁。
- 规则：IR 不含电气类型与 no-connect 标记，`unconnected-pin` 只能用引脚名排除 NC；`undriven-power`
  与 `missing-decoupling` 用引脚名 + 位号前缀判断角色；`net-name-conflict` 只能检出「同名不连通」，
  检不出「同一网络多个名称」。要更精确需要 IR 增加电气类型 / 别名 / 坐标，属 P2 讨论范围。
- 固件比对：只解析 §7.1 列出的构造。devicetree 的 `pinctrl-0 = <&label>` 间接引用与 `NRF_PSEL(...)`
  跳过；字母端口按 `P<口><脚>` 比对，数字端口控制器（`&gpio1 13`）按 `P<口>.<脚>` 与 `GPIO<脚>` 两种
  候选匹配 IR 引脚名（同一控制器的两种写法用信号名消歧）；`.ioc` 的通用 GPIO 无标签时跳过。
  固件内部冲突与引脚声明冲突都按「排序在后的那条声明」给出（码点序，保证跨平台一致），
  因此「原理图独有」条目的 `file` 是原理图源文件、`line` 省略。

## 待定（需要时按规格补充）

- `correction` 元素的 `confidence` 取值：目前人工修正记 1、Agent 修正保留原置信度（新建元素取 0.5）。
- 网络 id 的稳定性依赖「名字或首个引脚锚点不变」；结构性改动后引用旧 id 的修正会变 orphaned（符合 §3.4）。
- 识别评测（§10.4）的口径在 `scripts/lib/circuit-recognition-metrics.mjs`（离线脚本，不进 CircuitService）。
