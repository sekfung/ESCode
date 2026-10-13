# 电路分析（Circuit）

- 状态：Draft（P1 规格）
- 日期：2026-10-12
- 相关 ADR：[0005 Circuit IR 由 Host 服务持有](../adr/0005-circuit-ir-owned-by-host-service.md)

## 1. 目标与范围

在 ESCode Desktop 中分析原理图与 PCB，服务三类场景：

| 场景         | 说明                                                    |
| ------------ | ------------------------------------------------------- |
| A 设计评审   | 硬件工程师查找设计反模式与选型风险，并对照固件引脚定义   |
| B 读懂电路   | 按功能模块拆分（电源、MCU、接口等）并解释作用与信号流向 |
| C 软硬件联调 | 对照固件引脚定义与外设配置做一致性检查                  |

### 分期

| 阶段   | 输入                                                                         | 能力                                                   |
| ------ | ---------------------------------------------------------------------------- | ------------------------------------------------------ |
| **P1** | KiCad（`.kicad_sch` / `.kicad_pcb`）、网表、BOM | 结构化提取、电路解释、固件比对、电路面板 |
| P2     | Altium、Eagle、立创 EDA、Gerber/ODB++          | PCB 层面分析                            |

本文只规定 P1。P2、P3 开工前另行补充规格。

### 非目标（P1）

- 不支持纯 CLI/TUI：没有 Desktop Host 时工具返回 `unavailable`，不提供 CLI 内的降级引擎。
- 不支持手机端和 Web 客户端：遇到 `::circuit` 引用时只显示静态降级卡片。
- 不联网：不访问第三方器件库或在线 datasheet，也不下载模型。只会调用用户配置的模型服务。
- 不做完整的原理图编辑器，面板只支持修改属性和增删连接。

## 2. 产品规则

1. **结构化 IR 是唯一事实来源。** 电路解释、固件比对和面板展示都只读取 Circuit IR，不让模型绕过 IR 直接下结论。
2. **确定性解析得到的元素置信度为 1。** 来源包括 KiCad、网表、BOM。
3. **低置信度必须可见。** Agent 新建的元素（新网络、功能模块）记 0.5；面板以醒目颜色标出低置信度元素，报告中注明哪些结论依赖低置信度数据。
4. **不联网的选型检查。** 选型风险只依据模型自身知识和用户提供的本地 datasheet，报告注明"未联网核实"。
5. **导出物不会被读回。** `circuit_export` 写入 workspace 的快照标注为派生产物，服务永远不读取它。

## 3. 架构

### 3.1 组件与所有者

| 组件             | 位置                                                                           | 职责                                                                                              |
| ---------------- | ------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------- |
| `CircuitService` | `packages/services/src/circuit/`                                               | **IR 的唯一所有者**：解析、校验、合并修正、revision、持久化、监听源文件、固件解析与比对 |
| 协议与 schema    | `packages/shared/src/circuit/`，`packages/shared/src/escode-protocol/index.ts` | IR、工具请求/响应、事件的严格类型与运行时校验（zod）                                              |
| circuit 内置插件 | `apps/escode-cli/packages/circuit-plugin/`                                     | MCP server，作为薄 broker 客户端（结构同 `serial-plugin`），附带 `circuit` skill                  |
| 电路面板         | `packages/ui`（组件 + `hooks/useCircuit*`）                                    | 渲染、选择、高亮、修正、生成引用 chip；不持有 IR 事实                                             |
| 远程访问         | `packages/client/src/remoteServiceAccess.ts`                                   | 远程 workspace 调用 `CircuitService`                                                              |

依赖方向：UI → hooks → `CircuitService`（经服务访问层）；插件 → broker → `CircuitService`。`CircuitService` 不依赖 runtime 的具体实现，Main 不持有 circuit 状态。

### 3.2 IR 标识与持久化

- **IR key**：`identityKey = workspaceIdentity?.trim() || workspacePath`，再加上 workspace 内的源文件相对路径。
- 持久化位置：Host 应用数据目录 `circuits/<hash(identityKey)>/<hash(sourcePath)>/`，内容包括：
  - `ir.json`：当前 IR（含 `revision`）
  - `corrections.json`：修正记录（作者、会话 id、时间、目标元素、修正内容）
- 所有文件 IO 都用异步接口。

### 3.3 写入与事件顺序

```
面板(hook) / Agent(MCP→broker)
        │ write{ irId, baseRevision, ops }
        ▼
  CircuitService ── baseRevision ≠ revision ──▶ 拒绝 stale_revision（调用方重读后重试）
        │ 校验 ops → 应用 → revision++ → 异步持久化完成
        ▼
  emit circuit.changed{ irId, revision, stale }
        │
        ▼
  桌面面板：若 event.revision > 本地 revision → get(irId) 刷新
```

- 只有持久化完成后才发出事件，事件里的 `revision` 一定能读到。
- 写入按 `irId` 串行执行。同一 `irId` 的事件按 `revision` 单调递增，面板忽略小于等于本地 revision 的事件。
- 不靠超时掩盖竞争，也不做静默合并：任何写入的 `baseRevision` 落后都会被拒绝。

### 3.4 源文件变化

```
源文件变化 ──watch──▶ 标记 stale=true，emit circuit.changed（revision 不变）
                              │
              用户/Agent 请求重新解析 parse(irId, baseRevision)
                              ▼
            重新解析 → 按元素 id 重新套用修正 → 套用不上的修正标记 orphaned → revision++
```

- 不在后台自动重新解析。
- 面板显示 stale 提示和"重新解析"操作；失效的修正（orphaned）在面板中列出，供用户确认或删除。

### 3.5 平台差异

| 环境                   | 行为                                                                                              |
| ---------------------- | ------------------------------------------------------------------------------------------------- |
| Desktop 本地 workspace | 窗口内的 Local Host 运行 `CircuitService`                                                         |
| Desktop 远程 workspace | 持有该 workspace 的 Host 运行；请求贯穿携带 `workspaceIdentity` 与 `remoteSessionId`              |
| 纯 CLI/TUI             | broker 连接材料缺失，所有工具返回 `unavailable`，提示"请在 ESCode Desktop 中使用"                 |
| Web 客户端 / 手机      | `::circuit` 显示静态降级卡片（标题、器件数、网络数、"仅 Desktop 可用"），不加载面板，也不订阅事件 |

运行时注册（TS CLI 与 Rust runtime 行为一致，与 serial 同构，见 `docs/specs/serial-agent-tools.md`）：

- 只有 Host 注入 `ESCODE_HOST_CIRCUIT=1` 且 `circuit` 官方插件启用时，运行时才注册内置 `circuit` MCP server；
  它最后合并，同名的用户或插件配置不能覆盖它。设置页 `mcp/list` 不列出该 server。
- broker 的 socket 与 token（`ESCODE_CIRCUIT_BROKER_SOCKET` / `ESCODE_CIRCUIT_BROKER_TOKEN`）只注入 circuit server 的 env。
  broker 拒绝子 agent，并把请求转成 `interaction/circuit*` 反向请求，带上会话的 workspace 身份与 `remoteSessionId`。
  电路工具都是短请求，没有取消方法。
- 免批准的工具（第 5 节「需要批准」为否的六个）并入权限 `allowedTools`；`circuit_export` 仍需批准。
- broker 自身的错误码：token 不符或子 agent 返回 `unavailable`，请求超过 1 MiB 返回 `too_large`，
  请求形状不合法返回 `invalidInput`，连接失败或 Host 未带 `error.data.code` 时返回 `io_error`。

## 4. Circuit IR（v1）

```ts
interface CircuitIR {
  schemaVersion: 1;
  irId: string;
  revision: number;
  source: {
    kind: "kicad_sch" | "kicad_pcb" | "netlist" | "bom";
    path: string; // workspace 相对路径
    contentHash: string;
  };
  stale: boolean;
  components: Component[];
  nets: Net[];
  blocks: Block[]; // 功能模块划分，可由 Agent 通过修正补充
  diagnostics: IrDiagnostic[]; // 校验问题：悬空网络、位号重复、引脚数不匹配等
}

interface Component {
  id: string; // KiCad 使用其 UUID；网表 / BOM 用 `<kind>:<位号>`
  ref: string; // 位号，如 U3
  value?: string;
  partNumber?: string;
  footprint?: string;
  pins: { number: string; name?: string; netId?: string }[];
  origin: Origin;
}

interface Net {
  id: string;
  name?: string; // 如 I2C1_SDA
  pinRefs: { componentId: string; pin: string }[];
  origin: Origin;
}

interface Block {
  id: string;
  name: string;
  componentIds: string[];
  origin: Origin;
}

interface Origin {
  by: "parser" | "correction";
  confidence: number; // parser = 1；Agent 修正新建的元素 = 0.5，人工修正 = 1
  author?: { kind: "user" | "agent"; sessionId?: string };
}
```

规则：

- 所有字段在 `packages/shared` 中用 zod 定义，读写两侧都做运行时校验。
- `confidence` 只能由服务端写入。
- 一致性校验项：无重复位号、无单引脚网络、网络两端的器件都存在；校验不通过时写入 `diagnostics`。

## 5. MCP 工具契约

工具名前缀为 `mcp__circuit__`。所有响应都是 `{ ok: true, ... } | { ok: false, error: { code, message } }`。

| 工具                         | 输入要点                                                               | 输出要点                                          | 需要批准                          |
| ---------------------------- | ---------------------------------------------------------------------- | ------------------------------------------------- | --------------------------------- |
| `circuit_parse`              | `path`；已有 IR 时需要 `baseRevision`                                  | `irId`、`revision`、摘要                          | 否                                |
| `circuit_get`                | `irId`                                                                 | 摘要：器件数、网络数、模块、stale、低置信度元素数 | 否                                |
| `circuit_query`              | `irId`；按 `ref` / `net` / `block` 查询（省略过滤条件时列出该类全部，面板 BOM 与网络表即此），或 `trace` 追踪连接；`cursor` | 分页结果 + `nextCursor`                           | 否                                |
| `circuit_correct`            | `irId`、`baseRevision`、`ops[]`                                        | 新 `revision`                                     | 否（记录 author=agent，面板可见） |
| `circuit_compare_firmware`   | `irId`、固件文件路径列表                                               | 差异列表（引脚、网络、固件声明、来源行号）        | 否                                |
| `circuit_export`             | `irId`、目标路径                                                       | 写入的文件路径                                    | **是**                            |

- 单次响应有字节上限，超出部分通过 `cursor` 分页获取。
- 错误码：`unavailable`、`not_found`、`unsupported_format`、`invalid_ir`、`stale_revision`、`too_large`、`io_error`。
- `circuit` skill 规定 Agent 的工作流：先 `parse`，再 `get`，然后按需 `query`。解释或下结论时要引用位号和网络名，涉及低置信度数据时必须说明。

## 7. 固件比对

- 由服务端做确定性解析的格式：Zephyr/Linux devicetree（`.dts` / `.dtsi` / `.overlay`）、STM32CubeMX `.ioc`、C 头文件中的引脚宏（`#define XXX_PIN` / `GPIO_PIN_n` / `PORT` 组合）。
- 输出差异类型：引脚不一致、固件里有而原理图里没有、原理图里有而固件里没有、外设复用冲突。
- Arduino、PlatformIO 这类散落在代码中的引脚写法不经过比对器，由 Agent 自行推断，结果标为低置信度。

### 7.1 P1 解析范围

只解析下列构造，其它写法不猜（在报告中说明）：

- devicetree（`.dts` / `.dtsi` / `.overlay`）：`<&gpio<口> <脚> …>` 形式的 GPIO 描述符，属性名是
  `gpios` / `*-gpio` / `*-gpios` / `*-pins` 时取用；信号名取「属性前缀 + 节点名（有标签则用标签）」，
  如 `&i2c1 { sda-gpios = <&gpiob 7 …>; }` → `I2C1_SDA`。`pinctrl-0 = <&label>` 这类间接引用与
  `NRF_PSEL(...)` 忽略；字母端口按 `P<口><脚>` 比对，数字端口控制器（`&gpio1 13`）没有端口语义，
  按 `P<口>.<脚>` 与 `GPIO<脚>` 两种候选与 IR 引脚名比对（Nordic 与 ESP 系列各自命中，
  同一控制器的两种写法用信号名消歧）。
- STM32CubeMX `.ioc`：`P<口><脚>.Signal=` 行；值为通用 GPIO（`GPIO_Input` / `GPIO_Output` 等）时改用同组的
  `.GPIO_Label=` 作信号名，没有标签就跳过（没有可比的网络名）。
- C 头文件：`#define <前缀>_PIN`（值是 `GPIO_PIN_<n>` / `LL_GPIO_PIN_<n>`）配同前缀的
  `_GPIO_PORT` / `_GPIO_Port` / `_PORT` / `_Port`（值是 `GPIO<口>`），如 CubeMX 生成的 `main.h`。
- 比对以 MCU 引脚名为锚（`P[A-K]<脚>`、`P<口>.<脚>`、`GPIO<脚>`）：固件声明的信号名与原理图上该引脚的
  网络名不一致 → 引脚不一致；固件声明了原理图没有网络可对应的引脚 → 固件独有；原理图 MCU 引脚接了具名网络
  而固件没有声明 → 原理图独有；同一引脚在固件里被声明成两个信号、或同一信号落在两个引脚 → 外设复用冲突。
- 条目里的 `file` 与 `line` 指向固件声明所在位置（workspace 相对路径、1 起始行号）；
  「原理图独有」条目的 `file` 是原理图源文件（IR 不记录行号，`line` 省略）。

## 8. 电路面板（Desktop）

- **入口**
  - Agent 回复中独占一行的 `::circuit{"irId":"…","title":"…"}` 引用卡片，解析规则沿用 Gen UI 引用（代码块内不识别，消息完成后才渲染）。
  - 在文件树中打开 `.kicad_sch`、`.kicad_pcb`、`.circuit.json`（导出快照，只读展示）。
- **还没有 IR 时**：KiCad 文件通过 hook 直接请求 `CircuitService` 做确定性解析。
- **渲染**
  - KiCad：根据解析出的几何信息自己渲染 SVG。
  - 低置信度元素用醒目颜色标出。
- **交互**
  - 点击器件或网络，高亮整条连接，侧栏的 BOM 和网络表同步选中。
  - 修正：改位号、型号、值，增删连接。写入时带 `baseRevision`；遇到 `stale_revision` 时提示刷新，不覆盖数据。
  - 选中器件或网络后可"插入到输入框"，生成引用 chip（如 `U3`、`NET:I2C1_SDA`，附带 `irId`）。chip 只属于 renderer 端的输入草稿，不自动发送。
- **UI 规范**：遵守 `DESIGN.md`，复用已有组件，支持主题和国际化（zh-CN / en），日志使用 `packages/ui/src/logger.ts`。

## 9. 日志

- `CircuitService` 使用 `createServiceLogger("circuit")`。
  - `info`：解析完成（器件数、网络数、耗时）、stale 状态变化、导出。
  - `warn`：校验失败、修正失效（orphaned）。
  - `error`：持久化失败、broker 握手失败。
- 日志中不得写入电路内容、文件内容或真实用户数据，只记录计数和 id。

## 10. 验收

### 10.1 测试数据

- 3 块许可宽松的开源 KiCad 板子：MCU 最小系统、电源板、I²C 传感器板，在 `fixtures/README` 中记录来源与许可证。
- 固件样例：手写的 `.dts`、`.ioc`、头文件，故意埋入 2 到 3 处引脚不一致。
- 不使用任何真实客户的设计。

### 10.2 单元测试

- `services/circuit`：KiCad 解析；IR 合并；`stale_revision` 拒绝；按 UUID 重新套用修正与 orphaned；固件解析与比对；置信度只能由服务端给出。
- `shared/circuit`：IR 与工具 schema 的运行时校验（合法和非法输入）。
- `circuit-plugin`：工具契约，包括缺少 broker 时返回 `unavailable`、分页游标、错误码映射。
- `ui`：`::circuit` 引用解析（参照 `gen-ui/domain/references.test.ts`）。

### 10.3 E2E（`docs/test-cases/e2e/circuit-panel.feature`，wdio）

1. 在文件树中打开 `.kicad_sch` → 面板完成渲染 → 点击器件，所在网络高亮，BOM 同步选中。
2. 在面板中修改器件属性 → `revision` 加 1，修正记录 author=user。
3. 用过期的 `baseRevision` 写入 → 写入被拒绝，面板提示刷新，没有数据被覆盖。
4. 在外部修改源文件 → 面板显示 stale 提示 → 重新解析，修正按 UUID 保留，找不到对应元素的修正标为失效。
5. 点击对话中的 `::circuit` 卡片 → 打开面板，定位到对应的 IR。
6. 选中器件后插入输入框 → 生成引用 chip，草稿没有发送。
7. Web 客户端里的 `::circuit` → 显示静态降级卡片。

由 Agent 驱动的流程（固件比对）用插件契约测试和服务层测试覆盖，E2E 不依赖真实模型。

### 10.5 P1 完成标准

- 本规格和 ADR 0005 已合入。
- 10.2、10.3 在 Windows、macOS、Linux 上通过。
- `pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed` 全部通过。
