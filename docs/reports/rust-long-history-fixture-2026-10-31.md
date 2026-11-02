# 大活跃历史离线基线（2026-10-31）

本文补齐 P1「性能长尾与真实工作负载」中可离线验证的**大活跃历史打开/分页/下一轮**部分。
它不替代真实供应商、流式稳定阶段 RPC 长尾或 Windows/macOS 原生测量。

## 方法

- 用例：`packages/services/tests/escode-cli-rust-long-history-bench.test.ts` 新增
  `ESCODE_BENCH_FIXTURE=1` 模式；仍由真实 TS `createSqliteSessionStore` 写入长历史，
  再分别启动 Node bundle 与 Rust runtime，交错采样。
- 大库只测打开/分页时设置 `ESCODE_BENCH_SKIP_TURN=1`，避免超过 mock provider 的上下文窗口。
- 命令：

  ```bash
  ESCODE_BENCH_FIXTURE=1 TSX_TSCONFIG_PATH=packages/services/tests/tsconfig.escode-cli-rust.json \
    node --import tsx --test --test-timeout=300000 --test-force-exit \
    packages/services/tests/escode-cli-rust-long-history-bench.test.ts
  ```

- 环境：Linux x86_64（16 vCPU）、Node v24.14.0、rustc 1.95.0、worktree `dev`（HEAD `01ebe258`，
  含未提交的 S2–S5/parity 改动）、Rust 为 debug 二进制（`target/debug`）。
- 指标：首次打开（Rust 含一次性 TS 导入）、二次打开（独立进程、已导入）、`rows` 单页读取、
  下一轮 mock 首输出与完成时间。`rows` 返回 200 行，是接口单页上限，不是整段历史的投影。

## 结果（5 轮 p50 / max，毫秒）

| 指标                     |        Node |      Rust |
| ------------------------ | ----------: | --------: |
| 首次打开（含一次性导入） | 2277 / 2391 | 685 / 738 |
| 二次打开                 | 2073 / 2105 | 358 / 367 |
| rows 单页（200 行）      |      5 / 13 |   20 / 20 |
| 下一轮 mock 首输出       |   553 / 568 | 166 / 172 |
| 下一轮 mock 完成         |   553 / 568 | 206 / 210 |

### 2000 轮 / 3 次交错

同一用例改用 `ESCODE_BENCH_HISTORY_TURNS=2000 ESCODE_BENCH_ROUNDS=3`，生成的 TS 库为 4.1 MiB：

| 指标                     |        Node |        Rust |
| ------------------------ | ----------: | ----------: |
| 首次打开（含一次性导入） | 2567 / 2629 | 7643 / 8636 |
| 二次打开                 | 2343 / 2361 |   623 / 627 |
| rows 单页（200 行）      |       5 / 5 |     22 / 28 |
| 下一轮 mock 首输出       | 1330 / 1425 |   551 / 607 |
| 下一轮 mock 完成         | 1331 / 1425 |   703 / 764 |

### 10000 轮 / 1 次（只测打开与分页）

同一用例改用 `ESCODE_BENCH_HISTORY_TURNS=10000 ESCODE_BENCH_ROUNDS=1 ESCODE_BENCH_SKIP_TURN=1`，
生成的 TS 库为 19,550,208 字节（18.65 MiB）。10k 轮历史已超过 mock provider 的上下文窗口，
因此本轮不发送下一轮请求：

| 指标                     | Node |   Rust |
| ------------------------ | ---: | -----: |
| 首次打开（含一次性导入） | 3898 | 222148 |
| 二次打开                 | 3678 |   1748 |
| rows 单页（200 行）      |    5 |     22 |

同一 10000 轮 / 18.65 MiB 库再用 release 二进制（`ESCODE_BENCH_RUST_BINARY=target/release/escode-cli-rust`，
`cargo build -p escode-cli-rust --release` 后）跑 1 次：

| 指标                     | Node | Rust release |
| ------------------------ | ---: | -----------: |
| 首次打开（含一次性导入） | 3890 |        61838 |
| 二次打开                 | 3762 |          409 |
| rows 单页（200 行）      |    6 |            5 |

### 20000 轮 / 1 次（2026-11-02，只测打开与分页）

同一用例改用 `ESCODE_BENCH_HISTORY_TURNS=20000 ESCODE_BENCH_ROUNDS=1 ESCODE_BENCH_SKIP_TURN=1
ESCODE_BENCH_RUST_BINARY=apps/escode-cli-rust/target/release/escode-cli-rust`，生成的 TS 库为
39 MiB。环境与上表相同；20k 轮同样超过 mock provider 上下文窗口，不发送下一轮：

| 指标                     | Node | Rust release |
| ------------------------ | ---: | -----------: |
| 首次打开（含一次性导入） | 5303 |       253246 |
| 二次打开                 | 5082 |          742 |
| rows 单页（200 行）      |    5 |            6 |

### 50000 轮 / 1 次（2026-11-02，只测打开与分页）

同一用例改用 `ESCODE_BENCH_HISTORY_TURNS=50000 ESCODE_BENCH_ROUNDS=1 ESCODE_BENCH_SKIP_TURN=1
ESCODE_BENCH_RUST_BINARY=apps/escode-cli-rust/target/release/escode-cli-rust`，生成的 TS 库约 98 MiB：

| 指标                     |  Node | Rust release |
| ------------------------ | ----: | -----------: |
| 首次打开（含一次性导入） | 10490 |      1654037 |
| 二次打开                 | 10256 |         1704 |
| rows 单页（200 行）      |     5 |            6 |

50k 轮首次导入约 27.6 分钟；相对 20k 轮，轮数为 2.5 倍而导入耗时约 6.5 倍，呈超线性增长。
二次打开仍为 1.7s（约为 Node 10.3s 的 1/6.0），单页读取保持毫秒级。

## 读法

- 大历史冷进程打开：Rust 二次打开约为 Node 的 1/5.8；首次打开包含一次性导入，仍约为 1/3.3。
- 2000 轮时一次性导入成本占主导：Rust 首次打开 7.6s，Node 2.6s；二次打开 Rust 0.62s、Node 2.34s（约 1/3.8）。
- 单页读取两侧都在毫秒级；本轮 Rust 略慢（20ms vs 5ms），绝对差异小，不能据此推断分页实现优劣。
- 下一轮数字只反映本地 mock provider 与交付链路开销，不代表真实供应商或流式长尾。
- Rust 使用 debug 二进制，release 的绝对值会不同；本表只用于同机相对比较和回归基线。
- 10000 轮时 Rust 首次打开约 222s，仍由 debug 二进制的一次性 TS 导入主导；二次打开 1.7s，
  约为 Node 3.7s 的 1/2.1。导入成本随轮数明显增长，不能把首次打开当作稳态打开成本。
- release 二进制把 10000 轮首次导入从 222s 降到 62s，二次打开从 1.7s 降到 0.41s（约为 Node 的 1/9）。
  首次导入仍是单次成本；用户感知的稳态打开应以二次打开为准。
- 20000 轮 / 39 MiB 的 release 二次打开为 742ms（Node 5082ms，约 1/6.8）；一次性导入 253s 仍随轮数线性增长，
  是当前唯一明显长尾。单页读取仍为毫秒级。
- 50000 轮 / 98 MiB 的 release 二次打开为 1704ms（Node 10256ms，约 1/6.0），单页读取仍为毫秒级；
  一次性导入增至约 27.6 分钟。导入耗时相对轮数超线性（2.5 倍轮数约 6.5 倍耗时），是当前最值得修复的长尾。

## 仍缺

- 真实 TS runtime / 真实供应商的下一轮与流式稳定阶段 RPC 长尾。
- Windows/macOS 原生大历史测量（当前只有 Linux x64）。
- 真实安装库与 10 万轮级/真实用户大库；磁盘容量/备份保留策略的实测。
