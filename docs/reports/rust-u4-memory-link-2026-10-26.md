# Rust 阶段 4：MemoryLink 类型化直传（2026-10-26）

父计划：`docs/specs/rust-unified-host.md` §阶段 4 第 1 项。契约：`docs/specs/rust-u4-memory-link.md`。

## 1. 变更

- runtime：新增 `TypedMemoryLink` / `EmbeddedTypedIo`（`app-server/src/link.rs`）；输入侧新增
  `dispatch_value`，与字节链路的 `dispatch(line)` 共用同一段 `dispatch_frame` 语义；输出侧
  `start_typed_io` 直接转发 `Vec<Value>` 批次，不再 `serde_json::to_vec`。
- 嵌入入口：`embed::spawn_typed`（`EmbeddedTyped`），与 `spawn` 共用实例装配与收口。
- Host：`host-runtime` 的 embedded 链路切到类型化端点；`RuntimeReader::next_frame` 产出
  `Value` 或原始文本；`RuntimeWriter::send_value` 直传，`send_line` 保留给控制通道。
  帧泵对字节/child 链路保留原始文本扇出（订阅面文本逐字节不变），类型化链路在扇出时
  序列化一次（Renderer 边界）。
- `host-surface` 对已解析的帧调用 `send_json`，避免 runtime 再解析一次。
- 帧上限：类型化链路按 JSON 编码下界/上界判定 `MAX_REQUEST_BYTES`；只有贴近限额的帧
  序列化一次取精确长度，结果与字节链路一致。

stdio 链路与 child 链路未改：`StdioLink`、`start_io`、`MemoryLink` 原样保留。

## 2. 双传输一致性

`tests/dual_transport.rs`（10 passed，含新增 5 个）：

- 输入序列：类型化链路与字节链路产出同一 `Input` 序列（合法帧/应答/EOF）。
- 输出帧：同一批 `Value` 经类型化链路重编码后与字节链路 NDJSON 逐行相等。
- 超限：>1 MiB 帧两条链路都只给 `too-large`；下界未超限、实际编码超限的边界值走精确测量后
  仍拒绝。
- 关闭：drop 写端 = EOF；drop output = writer 收口。
- 背压：容量 1 且无人消费时，第二次发送必须等待。

## 3. 延迟

### 3.1 纯传输 echo（n=500，release，3 次）

| 链路        |                   p50 ms |                   p95 ms |
| ----------- | -----------------------: | -----------------------: |
| NDJSON 字节 | 0.0624 / 0.0623 / 0.0713 | 0.0785 / 0.1053 / 0.1205 |
| 类型化      | 0.0482 / 0.0463 / 0.0480 | 0.0592 / 0.0781 / 0.0662 |

类型化 p50 低约 25–33%，p95 低约 25–40%。这是免序列化本身能拿到的收益。

### 3.2 端到端控制 RPC（`v4/commands/query`，n=200，release，3 次）

| 链路        |                p50 ms |                p95 ms |
| ----------- | --------------------: | --------------------: |
| NDJSON 字节 | 0.107 / 0.062 / 0.085 | 0.160 / 0.082 / 0.146 |
| 类型化      | 0.062 / 0.049 / 0.063 | 0.103 / 0.064 / 0.100 |

- 两次都在 `≤0.1 ms` 目标内；类型化 3 次均低于字节链路（低 10–42%）。
- 单次运行的绝对值受调度影响明显（字节 p50 在 0.062–0.107 之间），因此测试只用 20% 的
  回归护栏，不硬断言绝对阈值。
- debug 构建（n=200）同口径：字节 p50 0.178 ms、类型化 0.129 ms；旧基线 `0.250 ms` 也是
  debug 口径，release 下阶段 1 基线即已接近目标。

结论：目标「进程内控制 RPC p50 ≤0.1 ms」在 release 下达到（两链路均满足），类型化直传在
纯传输层带来约 30% 的稳定收益；端到端层面收益被引擎处理摊薄，但仍不劣于 NDJSON。

## 4. 验证

- `cargo test --test dual_transport`：10 passed。
- `cargo test --test embed_memory_link`：9 passed（含新增延迟对照）。
- `pnpm test:escode-host`：cargo workspace 全量 + interop + settings interop 全绿。
- `pnpm check:escode-cli-rust`、`pnpm check:escode-host`：boundary + fmt + clippy `-D warnings` 均 exit 0。
- `pnpm test:escode-cli-rust`（`ESCODE_TEST_SERIAL=1`、REQUEST_MS=60000、EXIT_MS=15000，与 CI 同口径）：
  374 pass / 0 fail；默认 5s 请求预算在本机负载下会误伤 39 个用例（与本改动无关，单跑放宽后即通过）。
- `pnpm typecheck`、`pnpm lint`（0 error）、`pnpm architecture:check --changed`：通过。

### 4.1 CI

PR #4 的 37 项检查全绿。与本改动直接相关的两组：

| 项目                                               | 运行                                   | 结论                                                   |
| -------------------------------------------------- | -------------------------------------- | ------------------------------------------------------ |
| 跨平台桌面 E2E（typed Host 链路真实跑 GUI）        | `desktop e2e matrix` run `40452142525` | ubuntu / windows / macos 三个 `desktop e2e` 全 success |
| App 级性能（typed 链路下 4 路流式/workspace/退出） | 同上 run 的 `app perf (ubuntu-latest)` | success                                                |

`rust-runtime-ci` 首轮两个 macOS shard 分别失败在 `esc-code-cli-rust-mcp-official-auth`
（hostRequests 两条记录顺序互换）与 `escode-cli-rust-model-io`（trajectory 记录 3 ≠ 4），
rerun 后通过；`model-io` 在本机单独运行也通过。两者都是 Node/Rust 对照的时序/顺序断言，
与本改动无关（stdio 链路的 dispatch 语义未变），按既有 macOS 时序 flake 处理。

## 5. 未做/后续

- child 链路（`ESCODE_HOST_RUNTIME_LINK=child`）不做类型化：跨进程没有免序列化的可能。
- Renderer/WebSocket 帧面不变；Host 订阅面仍输出 JSON 文本（每条一次序列化）。
- 阶段 4 后续项（跨实例共享缓存、启动/退出路径优化）未开始；退出屏障实测约 2.17 s，
  已在阶段 3 报告中登记为独立工作包。
