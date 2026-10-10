# 串口调试器第三期：多串口、DTR/RTS、快捷指令、循环发送、校验和与分帧、波形

2026-10-10。在第一期（`docs/specs/serial-port-debugger.md`）与第二期（`docs/specs/serial-agent-tools.md`）之上扩展。
按以下顺序交付，每项单独提交：多串口 → DTR/RTS → 快捷指令 → 定时循环发送 → 校验和与分帧 → 波形。

## 1. 多串口

### 规则

- 每个窗口最多同时 4 个串口会话（`opening`/`open`/`closing`/`disconnected`/`error` 都计数，`closed` 不计）。
  超过时 `open` 返回 `invalidInput`：“已达到同时打开串口的上限”。
- 会话以**串口路径**为键：操作系统对串口独占，同一路径只能有一个会话，不另设会话 ID。
- 每个会话各自持有：1 MiB 环形缓冲、状态机、自动重连、参数、DTR/RTS 状态、循环发送任务。会话之间互不影响。
- `seq` 由 Service 在窗口内统一分配：跨会话、跨关闭重开都单调递增（同一会话内不连续）。
  这样同一串口关闭后重开，renderer 也不会把新数据误判为快照内旧数据。
- 关闭的会话保留在会话表中（便于查看历史，与第一期关闭后仍可见收发记录一致），但不计入上限、
  不出现在 `listSessions`；会话表最多保留 8 个会话，超出时淘汰最早关闭的会话。
- 热插拔轮询在“有可见面板”或“任一会话处于 disconnected”时运行，一次轮询服务所有会话的重连。

### 所有者与接口

唯一所有者仍是窗口级 Desktop Local Host 的 `SerialService`，内部改为 `Map<path, SerialSession>`。

```ts
interface ISerialService {
  list(): Promise<SerialPortInfo[]>;
  listSessions(): Promise<SerialSessionSummary[]>; // { path, status, config }
  open(params: { path: string; config: SerialConfig }): Promise<void>;
  close(params: { path: string }): Promise<void>;
  write(params: {
    path: string;
    bytes: Uint8Array;
    source: SerialSource;
    sessionId?: string;
  }): Promise<{ seq: number }>;
  clear(params: { path: string }): Promise<void>;
  getSnapshot(params: { path: string }): Promise<SerialSnapshot>;
  setSignals(params: {
    path: string;
    dtr?: boolean;
    rts?: boolean;
    pulse?: "esp32" | "arduino";
  }): Promise<SerialSignals>;
  startLoop(params: {
    path: string;
    bytes: Uint8Array;
    intervalMs: number;
    count?: number;
  }): Promise<void>;
  stopLoop(params: { path: string }): Promise<void>;
  setWatching(params: { watching: boolean }): Promise<void>;
  onData: Event<SerialChunk & { path: string }>;
  onStatus: Event<SerialStatus & { path: string }>;
  onPorts: Event<SerialPortInfo[]>;
}
```

- `SerialSnapshot` 增加 `signals: { dtr: boolean; rts: boolean }` 与 `loop?: { intervalMs; count?; sent }`。
  `SerialStatus` 增加 `loop?`（循环进度随状态事件推送，间隔不低于 250 ms，避免高频事件）。
- 不保留第一、二期的单会话签名：调用方只有仓库内的面板与 Agent 处理器，同步修改。
- 对未打开的路径调用 `write`/`setSignals`/`startLoop` 返回 `notOpen`；`getSnapshot` 返回 `closed` 空快照。

### 面板

- 侧边面板标签 `serial` 改为可多开：`id = serial:<uuid>`，标签状态记录绑定的 `path`（未绑定时为空）。
  标题为串口名（如 `COM3`），未绑定时为 `串口调试`：多个标签并排时标题会被截断，只显示串口名才能区分，
  USB 图标已表明是串口标签。
- “新建标签 → 串口调试”与命令面板入口都新开一个未绑定标签。
- 未绑定标签的串口下拉中，已有会话的串口标为“运行中”；选择它即挂回该会话（不重新打开）。
- 新标签的预选必须等串口列表与会话列表都加载完成后进行：上次用过且未被占用的串口优先，否则第一个空闲串口。
  已被其他标签绑定的运行中会话同样可选，两个标签显示同一会话。
- 关闭标签不关闭串口。应用重启后标签恢复为未绑定、预选上次的 `path`，不自动连接。
- 标签只订阅绑定 `path` 的事件；快照与事件衔接规则不变（按会话 `seq` 去重）。

### Agent

- 所有串口工具增加可选 `path`。省略时：恰好一个会话时使用它；没有会话时 `list`/`read` 返回空、其余返回 `notOpen`；
  多于一个会话时返回 `invalidInput`，消息列出当前会话路径，要求指定 `path`。`serial_open` 必须给 `path`（已如此）。
- `serial_list` 返回 `{ ports, sessions }`。不抢占规则按路径生效。
- 面板 `[Agent·…]` 标注不变。

## 2. DTR/RTS

- 面板两个开关，仅会话 `open` 时可用；开启 RTS/CTS 流控时 RTS 开关禁用。
- 状态由会话持有，经 `port.set({ dtr, rts })` 生效；打开串口后的初值为 `dtr=true, rts=true`（与 serialport 默认一致），
  并在快照中返回。自动重连后按断开前的状态恢复。
- 复位脉冲（下拉按钮）：
  - ESP32（esptool 经典复位）：`DTR=0,RTS=1` → 100 ms → `DTR=1,RTS=0` → 50 ms → `DTR=0`；
  - Arduino：`DTR=1` 保持 100 ms → `DTR=0`。
    脉冲结束后恢复到脉冲前的开关状态。脉冲期间再次调用 `setSignals` 排队到脉冲结束后执行。
- Agent 新增 `serial_set_signals`：`{ path?, dtr?, rts?, pulse? }`，需审批（`destructiveHint`）；
  `pulse` 与 `dtr/rts` 互斥。审批卡片显示目标串口与变化（如 `DTR 0→1`、`ESP32 复位脉冲`）。
  新增反向方法 `interaction/serialSetSignals`；TS 与 Rust broker 增加该方法映射。

## 3. 快捷指令

- 全局列表，存设置 `serialQuickCommands: SerialQuickCommand[]`，最多 100 条：
  `{ id, name, data, mode: "text" | "hex", lineEnding, checksum? }`（`checksum` 见第 5 节）。
- 面板发送区上方一行按钮，点击即发送到当前标签的串口；可拖拽排序（复用 `@dnd-kit/sortable`）、新增、编辑、删除。
- 导入/导出 JSON 文件：导出走 `IPlatformService.saveFile`（导出内容不含 id）；导入使用渲染层原生文件选择
  （`<input type="file">` + `File.text()`），无需经 Host 读文件。按 schema 校验，非法条目跳过并提示数量；
  导入为追加并重新分配 id，超过上限的部分丢弃并提示。
- 可作为定时循环发送的内容。

## 4. 定时循环发送

- 由 Host 的会话调度，不在 renderer：关闭标签或切换视图不影响节奏。
- 参数：内容（发送框当前内容或某条快捷指令，renderer 编码为字节后提交）、间隔 ≥ 10 ms、次数（不填为无限）。
- 启动后立即发送第一次，之后按间隔发送。每次发送等同一次 `write(source="user")`，进入收发记录。
  上一笔写入未完成时跳过本次 tick 不排队，避免积压；写入失败时停止。
- 串口关闭、断开、出错或调用 `stopLoop` 时停止；每个会话同一时刻最多一个循环任务，再次启动替换旧任务。
- Agent 不能启动循环发送。

## 5. 校验和与分帧

### 发送校验和

- 算法：XOR（BCC）、SUM8、CRC-8（多项式 0x07）、CRC-16/MODBUS、CRC-16/CCITT-FALSE、CRC-32、LRC。
- 覆盖范围：内容字节（不含行尾），可选跳过开头 N 字节；结果附加在内容之后、行尾之前。
- 多字节结果的字节序：大端或小端（CRC-16/MODBUS 惯例为小端，默认按算法惯例）。
- 配置位于发送栏（标签内状态），快捷指令可各自携带一份（`checksum` 字段，随导入导出保留）。

### 接收分帧

- 三种方式：分隔符（文本转义如 `\r\n`，或 HEX 如 `0D 0A`）、固定长度、时间间隔（两段数据间隔超过 N ms 即新帧，
  使用 Host 记录的 chunk 时间戳）。只作用于 RX；TX 行保持原样。
- 启用分帧后，文本/HEX 显示按帧换行；可选“校验帧”：去掉分隔符后，以末尾 k 字节为校验和（k 由算法决定），
  覆盖范围同样遵守“跳过开头 N 字节”；校验失败的帧以 `text-destructive` 标红并显示期望值。
- 分帧与校验只在 renderer 中进行，不改变 Host 缓冲与导出（导出保持原始 chunk）。
- 帧显示时去掉分隔符；分隔符/固定长度模式下尾部未结束的数据显示为浅色“未完成帧”，不参与校验。
  时间间隔模式没有结束标记，尾帧视为完整并立即校验（不依赖定时器）。
- TX 每个 chunk 一行，不打断正在拼接的 RX 帧；RX 帧在完成时按顺序排在其后。
- 分帧设置（开关、方式、分隔符/长度/间隔、帧校验）属于标签内的显示状态，不持久化；参数非法时退回普通显示并提示。
- 分隔符文本转义支持 `\r` `\n` `\t` `\0` `\\` `\xNN`；长度 1–4096 字节，间隔 1–60000 ms。
- 不做具体协议解码器（如 Modbus 寄存器解析）。

## 6. 波形

- 显示模式增加“波形”：从 RX 解码文本中按行解析。
  - 纯数字列：逗号、空格或制表符分隔，第 i 列为曲线 `ch<i>`；
  - 键值对：`名称:值` 或 `名称=值`，以空白或逗号分隔，每个名称一条曲线。
  - 无法解析的值跳过，不中断曲线；空行忽略。
- 最多 8 条曲线（按首次出现顺序），每条保留最近 2000 个点；横轴为相对首个点的秒数（Host 时间戳）。
- 每行的时间取该行结束所在 chunk 的 Host 时间戳；每条曲线超过 2000 点时淘汰最早的点。
- 暂停（冻结显示，数据继续累积）、清空、导出 CSV（`t,<曲线名...>`，缺失值留空）。三者复用日志工具栏的同一组按钮：
  暂停冻结传给波形的 chunk，恢复后增量补齐；清空与日志一致（清空该串口的 Host 缓冲）；波形模式下导出按同一规则重新解析完整缓冲生成 CSV。
- 曲线颜色复用图表色板（6 色），第 7、8 条曲线用虚线区分；切换编码时重新解析。
- 绘图使用 `components/ui/chart.tsx`（recharts），重绘节流为最多每秒 10 次；解析增量进行，不对整个缓冲重复解析。

## 验收

每一项交付都包含对应的单元测试，交互改动补充 `docs/test-cases/e2e/serial-port-debugger.feature` 场景，
并在开发版上以 `ZCODE_SERIAL_MOCK_PORTS` 虚拟串口实际驱动验证：

- 多串口：两个虚拟串口同时打开、各自收发互不串扰、第 5 个返回上限错误；关闭标签后挂回运行中会话；
  Agent 工具缺省 `path` 的三种情况；整链路测试覆盖带 `path` 的调用。
- DTR/RTS：开关状态进入快照；脉冲按时序调用 binding `set` 并恢复；流控开启时 RTS 禁用；
  `serial_set_signals` 经 TS 与 Rust broker 转发，审批预览正确。
- 快捷指令：增删改排序持久化；导入时非法条目跳过；点击发送的字节与发送栏一致。
- 循环发送：次数与间隔准确（容差内）；关闭/断开即停止；不积压。
- 校验和：每个算法用公开的标准测试向量（`"123456789"` 的 check 值）验证；附加位置、跳过字节与字节序；
  三种分帧方式与帧校验失败标红。
- 波形：数字列与键值对解析、上限截断、节流、CSV 导出。
