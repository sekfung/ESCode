# Rust 阶段 4：MemoryLink 类型化直传（rust-unified-host.md 超越项 1）

父计划：`docs/specs/rust-unified-host.md` §阶段 4「进程内免序列化直连」。
前置：`docs/specs/rust-u1-runtime-lib.md` §3.3（双传输）与 `rust-u2-host-skeleton.md` §3（runtime-link）。

## 1. 问题与目标

现状（阶段 1 预留、阶段 2–3 沿用）：嵌入实例经 `MemoryLink` 传 NDJSON 字节，链路两端各自
`serde_json` 一次：

```
Host caller ── Value → to_string（序列化 1）──▶ 有界字节通道 ──▶ runtime parse（解析 1）──▶ Input
runtime Output ── Value → to_vec（序列化 2）──▶ 有界字节通道 ──▶ Host parse（解析 2）──▶ 帧泵
```

每帧 2 次序列化 + 2 次解析都发生在同一进程内，没有管道编码的收益。

目标：嵌入链路改为 `serde_json::Value` 直传，消除上述 4 次转换；stdio 链路逐字节不变；
Host 订阅面仍输出 JSON 文本（Renderer 边界必须序列化，只做一次）。

指标：进程内控制 RPC p50 ≤ 0.1 ms（基线 0.250 ms / p95 0.334 ms，n=50，见
`docs/reports/rust-u1-runtime-library-2026-10-14.md`）。

## 2. 范围与非目标

范围内：

- `apps/escode-cli-rust/crates/app-server/src/link.rs`：新增 `TypedMemoryLink` /
  `EmbeddedTypedIo`，与既有 `MemoryLink` 并存。
- `apps/escode-cli-rust/src/embed.rs`：新增 `spawn_typed`；`spawn`（字节链路）保留给差分与诊断。
- `apps/escode-host/crates/host-runtime/src/{link,instance,instance_pump}.rs`：embedded 链路改用
  类型化端点；child 链路（stdio 子进程）与行为保持不变。

非目标：

- Renderer/WebSocket 帧面不变（`host-surface` 仍发 JSON 文本）。
- child 链路（`ESCODE_HOST_RUNTIME_LINK=child`）不做类型化：跨进程没有免序列化的可能。
- 不删 `MemoryLink`：双传输一致性与故障注入仍用它。

## 3. 状态所有者与数据流

```
                    ┌──────────────── runtime（同进程）────────────────┐
Host caller ─Value─▶│ mpsc<Value>(cap 64) ─▶ dispatch_value ─▶ Input    │
                    │                                                   │
Host 帧泵 ◀─Value──│ mpsc<Vec<Value>>(cap 64) ◀─ Output(mpsc<Vec<Value>>)│
                    └───────────────────────────────────────────────────┘
Host 订阅面：Value → to_string 一次 → Renderer
```

- 输入所有权：调用方构造 `Value` 后交给通道；runtime 侧 `dispatch_value` 是唯一解释器，与
  `dispatch(line)` 共用同一段语义（`dispatch(line)` = 解析 + `dispatch_value`）。
- 输出所有权：runtime 的 `Output`（`mpsc<Vec<Value>>`）仍是唯一写者；类型化 writer 只做
  「批量转发」不做编码。批量边界 = runtime 的提交边界，逐条保持顺序。
- 帧泵：仍是读端唯一消费者；`Value` 分支与现有逐行分支走同一套分流（应答/反向请求/扇出）。

## 4. 契约

### 4.1 帧边界

- 一帧 = 一个 `serde_json::Value`；跨帧不会合并或拆分（通道元素即帧，批量元素即提交批）。
- `MAX_REQUEST_BYTES`（1 MiB）在类型化链路按**与字节链路完全相同**的判定执行：
  - 快速通过：若值的最坏编码上界（字符串按 6 倍转义 + 结构开销）≤ 限额，直接接受；
  - 快速拒绝：若值的最小编码下界（字符串原长 + 结构开销）> 限额，直接 `Input::TooLarge`；
  - 其余情况序列化一次得到精确长度（仅靠近限额的帧付出这次序列化）。
  - 该判定放在输入侧；输出侧不设上限（与现状一致：输出上限由 Host 的 RPC 边界处理）。

### 4.2 flush 与背压

- 通道有界（默认 64 帧/批），写满即阻塞，语义与 stdio 管道背压一致，不用超时兜底。
- 没有「半帧」状态：类型化链路不存在部分写；`flush()` 退化为每批交付即完成。
- 输出停滞看门狗（`watch_stalled_output`）仍按批次起止计时，语义不变。

### 4.3 关闭

- 调用方 drop `EmbeddedTypedIo`（写端）→ 入站通道关闭 → runtime 读到 0 帧 = EOF，走既有
  `Input::Eof` 收口路径。
- runtime 的 `Output` 发送端全部 drop → 出站通道关闭 → Host 帧泵 `next_frame()` 返回 `None`
  → 既有 `close()` + 崩溃回收路径。
- 顺序保证：同一批 `Vec<Value>` 内保序；批间保序；关闭信号在已入队批次之后才被观测到。

### 4.4 与字节链路的等价边界

- 出站文本等价：`serde_json::Value` 的 Map 在本工作区未启用 `preserve_order`（BTreeMap，键有序），
  因此 Host 对收到 `Value` 再 `to_string` 得到的字节与「runtime 序列化后传字节」一致。
- 入站等价：`dispatch_value` 与 `dispatch(line)` 共用实现，非法 JSON/空白行/`TooLarge`/
  `Eof` 的产出相同。
- 唯一有意差异：类型化链路没有「行」概念，帧上限按上节的上下界判定而不是字节长度实测；
  边界值（下界 ≤ 限额 < 实际上界）会退化为精确测量，结果仍一致。

## 5. 双传输一致性测试矩阵

| 场景                                     | StdioLink        | MemoryLink（字节） | TypedMemoryLink                                |
| ---------------------------------------- | ---------------- | ------------------ | ---------------------------------------------- |
| 输入序列（合法/空白/非法 JSON/无尾换行） | 现有             | 现有               | 新增：与两者产出同一批 `Input`                 |
| 出站帧序列与顺序                         | 现有             | 现有               | 新增：同一 `Output` 批次经两条链路得到同一文本 |
| >1 MiB 帧                                | 现有 `too-large` | 现有 `too-large`   | 新增：同样 `too-large`（上下界判定）           |
| 关闭语义（drop 写端 / drop 输出端）      | 现有             | 现有               | 新增                                           |
| 背压（满通道阻塞）                       | 现有             | 现有               | 新增：容量 1 时第二批必须等待                  |

## 6. 指标与测量

两层测量，避免把引擎处理算进传输差异：

1. **纯传输 echo**（`tests/dual_transport.rs::transport_only_round_trip_latency_typed_vs_bytes`）：
   echo 服务把每个请求原样回成应答帧，n=500。判定：typed p50 ≤ byte p50 × 1.10（回归护栏）；
   实测差值作为「免序列化收益」的证据。
2. **端到端控制 RPC**（`tests/embed_memory_link.rs::in_process_control_round_trip_latency_*`）：
   真实 Engine、`v4/commands/query`、n=200。判定：typed p50/p95 ≤ byte × 1.20（噪声护栏）；
   `≤0.1 ms` 的发布口径目标以 release 实测记录，不在 debug 构建上硬断言。

报告：`docs/reports/rust-u4-memory-link-2026-10-26.md`，含 debug 与 release 两组。

## 7. 分片与回退

1. runtime 侧 `dispatch_value` 重构 + `TypedMemoryLink` + 单元测试（不改 Host）。
2. Host embedded 链路切到 typed，child/stdio 不变；跑 `pnpm test:escode-host` 与
   `node scripts/test-escode-host-interop.mjs`。
3. 延迟基准与报告。
4. 回退：Host 的 `Link::Embedded` 改回 `embed::spawn`（一行），`ESCODE_HOST_RUNTIME_LINK=child`
   仍可整体回退到子进程链路。

## 8. 风险

| 风险                   | 对策                                                    |
| ---------------------- | ------------------------------------------------------- |
| 两条链路语义漂移       | `dispatch_value` 单一实现 + 双传输矩阵逐场景对拍        |
| 帧上限判定放宽         | 上下界判定 + 边界值精确测量；测试覆盖 1 MiB 边界        |
| 帧文本字节变化（键序） | 两侧都用默认 `serde_json`（BTreeMap）；测试直接比较文本 |
| 背压退化               | 有界通道 + 容量 1 的阻塞测试                            |
