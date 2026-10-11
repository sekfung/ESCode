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
- 元素 id：根图纸元素是 KiCad UUID，子图纸元素带实例路径前缀（`/<sheetUuid>/<uuid>`）；
  网表 / BOM 没有 UUID，用 `netlist:<位号>`、`bom:<位号>`。网络 id 由名字（global/电源/hierarchy/local 标签）
  或首个引脚锚点派生；同名 local / hierarchy 标签只在同一图纸实例内成网，所以这类网络 id 带作用域后缀。
  这一切都是为了让「重新解析后按 id 重新套用修正」成立。
- 修正记录（corrections.json）保留作者、会话 id、时间、目标元素与修正内容；重新解析后套用不上的记录
  保留在磁盘并标 `orphaned`，面板列出后由用户确认或删除（只能删除 orphaned 的记录）。
- `CircuitLayout` 是渲染几何的只读派生视图：与 IR 同一次解析产出、按 revision 对齐、不进 IR、不参与修正（§8）。
- 日志用 `createServiceLogger("circuit")`，只记 id、计数、耗时，不写电路内容与文件内容（§9）。

## P1 已知边界（按规格声明，不另加兜底）

- 总线成员不展开：`(bus)` 只记入 `limitations`，总线分支上的标签照常成网。
- 子图纸最多 64 个文件 / 8 层；层级成环与未加载的子图纸记入 `limitations`，不猜测内容。
- `.kicad_pcb` 的 layout 只给器件落点与板框尺寸，不画铜箔与走线。
- PDF / 图片不在这里解析（属于 `circuit_submit_recognition`，G6）。
- 同一个 workspace 被多个窗口 Host 同时打开时，revision 检查是各自服务内的乐观并发，不做跨进程锁。

## 待定（需要时按规格补充）

- `correction` 元素的 `confidence` 取值：目前人工修正记 1、Agent 修正保留原置信度（新建元素取 0.5）。
- 网络 id 的稳定性依赖「名字或首个引脚锚点不变」；结构性改动后引用旧 id 的修正会变 orphaned（符合 §3.4）。
