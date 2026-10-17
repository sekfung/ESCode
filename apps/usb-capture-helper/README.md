# escode-usb-capture

ESCode USB 抓包功能的**抓包引擎**：读 usbmon 二进制 API（`/dev/usbmonN`）把 URB 事件写成 pcapng，
交给 USB 插件的 `usb_decode` 解码。

> **状态（2026-10-14）**：产品决定**实时抓包只在 Windows 提供且随安装包分发**，Linux/macOS 只做离线解码
> （见 `docs/specs/usb-agent-tools.md` §4）。因此：
>
> - **Linux 部分（`usbmon_bin.rs`、`probe.rs`、`install_privileges.rs`、CLI 入口）已冻结**：不在产品路径、
>   不随任何安装包分发、插件也不会对 Linux 用户承诺它（只有抓包器确实存在时才被认出来使用）。
> - **可移植部分是留给 Windows 服务的底座**：`pcapng.rs`（写入器 + 记录模型 + 路径校验）与那套
>   「一行 JSON、固定失败码」的契约，Windows 服务作为本 crate 的第二个 bin target 复用，只新增
>   USBPcap 设备读取与命名管道服务端。
> - 冻结不代表腐烂：`pnpm check:usb-capture-helper` / `pnpm test:usb-capture-helper` 仍在 CI 门禁里跑。

- 插件调用方：`apps/escode-cli/packages/usb-plugin`（`--probe` 探测能力，抓包时拿到一行 JSON 结果）。
- 投放位置与本仓库既有约定同构：`resources/tools/escode-usb-capture`（与 `cua-helper` 一致），
  或插件包内 `bin/<platform>-<arch>/escode-usb-capture`；实际投放由打包链路完成，本目录只负责源码与构建。
- 本 crate 目前只有 Linux 的可执行入口；Windows 服务将复用它的库部分（见上面的状态说明）。

## 构建与验证

```bash
cargo build --release --offline --manifest-path apps/usb-capture-helper/Cargo.toml
cargo test  --offline --manifest-path apps/usb-capture-helper/Cargo.toml
cargo clippy --offline --manifest-path apps/usb-capture-helper/Cargo.toml --all-targets -- -D warnings
cargo fmt --manifest-path apps/usb-capture-helper/Cargo.toml --check
```

- 独立 crate：`Cargo.toml` 里有一个空的 `[workspace]` 表，因此不会被 `apps/escode-host` /
  `apps/escode-cli-rust` 的 workspace 认领，构建产物与 `Cargo.lock` 都落在本目录。
- 依赖只有 `libc`（ioctl/mmap/poll/sigaction）与 `serde_json`（stdout 的一行 JSON），
  两者都在本地 cargo 缓存里，`--offline` 可直接解析并构建；工具链按仓库 `rust-toolchain.toml`（Rust 1.95）。
- 产物是单文件、无运行时依赖（除 glibc）的可执行文件。

## CLI

stdout **恒为一行 JSON**；失败时打印 `{ ok: false, error: { code, message, remediation? } }` 并以非 0 退出码结束；
人类可读的说明（警告、内核丢弃计数读取失败等）走 stderr。

| 子命令                                                | 说明                                                                                                                   |
| ----------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| `--probe`                                             | 只探测：`{ ok, platform, available, usbmonModuleLoaded, privilege, interfaces: [{ id, bus }], reason?, remediation? }` |
| `--bus N --duration-ms M --max-bytes B --output PATH` | 抓包：`{ ok, packets, bytes, dropped, skippedIsochronous, stoppedBy, output, bus }`                                    |
| `--install-privileges`                                | 特权步骤（仅 pkexec/root）：`{ ok, group, udevRule, module, nodes }`                                                   |
| `-h`, `--help`                                        | 打印用法（唯一不是 JSON 的输出，且只在这个显式参数下出现）                                                             |

- `--bus` 1–255（`0` 是 usbmon 的「所有总线」聚合设备，不作为抓包目标）；对应 `/dev/usbmonN`。
- `--duration-ms` 1–60000（默认 10000），`--max-bytes` 1–536870912（默认 64 MiB），与 spec §3.3 的工具级默认值一致。
  `maxBytes` 是**软上限**：按事件粒度检查，最多超出单个事件的大小。
- `--output` 必须以 `.pcapng` 结尾（大小写不敏感），父目录必须存在且是目录，**拒绝符号链接**
  （输出文件本身与其直接父目录都拒绝，避免提权/参数化路径写到别处），不做任何解释器或命令转发。
- 收尾：先写 `<output>.partial`，正常结束时 flush + fsync 后改名为 `<output>`；**被 SIGINT/SIGTERM 中断时保留
  `.partial`**，此时 JSON 里的 `output` 就是那个 `.partial` 路径（不假装最终文件存在），`stoppedBy = "signal"`。
- 退出码：`0` 成功（含 `--probe` 报告 `available: false`），`2` invalidInput，`3` permission，
  `4` no-interface，`5` busy，`6` io。

示例（`--probe` 的输出是本机实跑结果；抓包那条是**真机上的预期形状**，本机没有 usbmon，未验证）：

```bash
$ escode-usb-capture --probe          # 本机实跑（无 usbmon、无特权）
{"available":false,"interfaces":[],"ok":true,"platform":"linux","privilege":"denied",\
 "reason":"usbmon 内核模块未加载（没有 /sys/module/usbmon，也没有 /dev/usbmonN）。",\
 "remediation":"加载模块后重试：`sudo modprobe usbmon`；也可以用 `pkexec escode-usb-capture --install-privileges` 一次性完成加载与授权。",\
 "usbmonModuleLoaded":false}

$ escode-usb-capture --bus 1 --duration-ms 5000 --max-bytes 67108864 --output /tmp/read.pcapng
# 有 usbmon 与权限时的预期输出（字段名已由 CLI 契约测试固定，数值取决于现场流量）：
{"bus":1,"bytes":20480,"dropped":0,"ok":true,"output":"/tmp/read.pcapng","packets":137,\
 "skippedIsochronous":4,"stoppedBy":"duration"}
```

## 权限模型：为什么不用 setcap

内核 `drivers/usb/mon/mon_bin.c` 在 open / ioctl / mmap 路径上**没有任何 capability 检查**，
能不能读完全取决于 `/dev/usbmonN` 的节点权限。因此本工具**不需要也不使用 `setcap`**，最小授权是
「让指定用户可读节点」：

1. udev 规则 `SUBSYSTEM=="usbmon", MODE="0640", GROUP="escode-usb"`（覆盖将来创建的节点）；
2. 组 `escode-usb` + 把发起安装的用户加入该组；
3. `modprobe usbmon` 并把 `usbmon` 写进 `/etc/modules-load.d/escode-usb.conf`；
4. 对当前已存在的节点执行 `chgrp escode-usb` + `chmod 640` 的 syscall 等价操作，让**当前会话立即生效**。

四条都在 `--install-privileges` 里，**不接受任何路径/用户名参数**：目标用户从 `PKEXEC_UID`（pkexec）
或 `SUDO_UID`（sudo）推导，两者都没有直接报错退出。系统工具（`groupadd`/`usermod`/`udevadm`/`modprobe`）
以 `Command` 直接执行、不经 shell，且只在可执行文件存在时尝试；`modprobe` 失败不致命（模块可能已加载、
或编进了内核），其余步骤失败即整体失败。全过程幂等：已完成的步骤重复执行成功而不是报错。

- 单独想手动修也可以（remediation 里的等价命令）：`pkexec escode-usb-capture --install-privileges`，
  或 `sudo usermod -aG escode-usb $USER` 后重新登录。
- `--probe` 完全不需要特权：`/sys/module/usbmon` 是否存在判断模块，试着只读打开第一个节点判断权限
  （`EACCES/EPERM` → `denied`）。
- `usbmon0`（聚合设备）不在 `interfaces` 里导出：抓包按具体总线进行，导出 0 会让上层把它当成合法总线。
- 若 usbmon 编进了内核（没有 `/sys/module/usbmon` 但有设备节点），`usbmonModuleLoaded` 以设备节点为准。

## pcapng 与 usbmon 的字段契约

写入必须能被插件读侧（`packages/usb-plugin/src/pcap/pcapng.ts` + `pcap/usbmon.ts`）原样解析，因此：

- SHB 28 字节、小端、1.0；IDB `linktype = 220`（`LINKTYPE_USB_LINUX_MMAPPED`）、snaplen 262144、
  带 `if_name = "usbmonN"` 选项（值按 4 字节对齐）；每个事件一个 EPB，`interface_id = 0`。
- EPB 时间戳 `ts_sec * 1_000_000 + ts_usec`（µs）按「高 32 位字段在前、低 32 位字段在后」写，
  **不是**把 u64 直接小端写出（后者会变成低字在前，30 万秒以上的时间戳就会被读侧还原错）。
- EPB 数据区 = 内核给的 **64 字节 `struct mon_bin_hdr` 原样** + 载荷；`cap_len = 64 + len_cap`、
  `orig_len = 64 + len_urb`（两者不等即短传输），整块 4 字节对齐。
- 等时传输（`xfer_type == 0`）**整体跳过**并计入 `skippedIsochronous`：它的 `len_cap` 含尾部 ISO 描述符，
  长度语义复杂，而这些描述符我们不解码——宁可如实跳过，也不写出长度可疑的数据。
- 环形缓冲里的填充包（`type == '@'`，内核用来防止记录跨缓冲末尾）跳过；记录本身不会跨缓冲末尾。
- 读取循环用 `poll(POLLIN)` + `MON_IOCX_MFETCH`（取偏移）**配对** `MON_IOCH_MFLUSH`（按条数释放）：
  MFETCH 只返回偏移、不消费环形缓冲，不释放的话下一轮会重复拿到同一批事件并把环塞满。
- 选项区不写 `opt_endofopt`：读侧在剩余不足 4 字节时结束扫描，`if_name = "usbmon6"` 时 IDB 正好 32 字节。
- 内核丢弃计数读一次 `MON_IOCG_STATS.dropped`（内核读取即清零，因此这就是「本次抓包期间因缓冲满丢弃的事件数」）；
  该 ioctl 失败时打印 stderr 警告并上报 `dropped = 0`，不静默掩盖。
- 头部按小端解析：`LINKTYPE_USB_LINUX_MMAPPED` 没有字节序标记，读侧本来就按小端解，
  因此本工具只支持小端 Linux（x86_64/aarch64/riscv64 等）。

## ⚠️ 未在真机验证的部分

开发/CI 环境**没有加载 usbmon、没有 `/dev/usbmon*`、也没有 root**，所以下面这些路径只经过编译与代码审阅，
**没有在真机上跑过**：

1. `src/usbmon_bin.rs` 的 `open` / `ioctl(MON_IOCQ_RING_SIZE / MON_IOCX_MFETCH / MON_IOCH_MFLUSH /
MON_IOCG_STATS)` / `mmap` / `poll` 循环（含环形缓冲偏移、填充包、批处理与释放配对）；
2. `SIGINT`/`SIGTERM` 收尾路径（保留 `.partial`、`stoppedBy = "signal"`）；
3. `--install-privileges` 的真实提权效果（组、udev 规则、节点属组）；
4. 内核丢弃计数在不同负载下的取值是否与内核 `cnt_lost` 一致。

其他部分已经真实跑过：pcapng 写入用**仓库里真实的 TS 读侧**（`pcapng.ts` + `usbmon.ts`，用仓库
`node_modules/typescript` 编译后在 node 里运行）交叉验证过——linktype 220、接口名、µs 时间戳、
控制传输 setup、批量短传输、`status = -32` 都逐字段还原正确，截断文件也按读侧预期返回
`truncated: true` 且已解析的前缀可用（等时记录与填充包没有出现在结果里，与跳过计数一致）。

真机复核步骤（需要 root 或已授权用户）：

```bash
sudo modprobe usbmon
cargo run --offline --manifest-path apps/usb-capture-helper/Cargo.toml -- --probe
# 期望：available: true、privilege: "ok"、interfaces 里能看到 usbmon1…
cargo run --offline --manifest-path apps/usb-capture-helper/Cargo.toml -- \
  --bus 1 --duration-ms 5000 --max-bytes 67108864 --output /tmp/read.pcapng
# 期望：packets > 0；抓包期间插拔一次设备后能用 Wireshark（usbmon 220）或 ESCode 的 usb_decode 打开
# 再验证中断收尾：Ctrl-C 之后 /tmp/interrupted.pcapng.partial 保留、stdout 的 stoppedBy 是 "signal"
```

真机复核时重点看三件事：**packets 数与 `usb_decode` 的记录条数一致**（若出现成片完全相同的重复 URB，
说明 `MON_IOCX_MFETCH`/`MON_IOCH_MFLUSH` 的配对语义写错了）；`dropped` 在不丢包时是 0；
`--bus` 与 `/dev/usbmonN` 的对应关系正确（`--probe` 的 interfaces 与 `lsusb` 的总线号对照）。

## 目录结构

```
apps/usb-capture-helper/
  Cargo.toml            # 独立包（空 [workspace]）、libc + serde_json
  src/main.rs           # 手写参数解析、一行 JSON 输出、退出码
  src/lib.rs            # 模块声明
  src/error.rs          # 错误码（permission/no-interface/busy/io/invalidInput）与 errno 分类
  src/pcapng.rs         # pcapng 写入器（纯函数 + 文件写入 + 路径校验）
  src/usbmon_bin.rs     # usbmon 头解析/事件归一化（纯逻辑）+ ioctl/mmap 薄层 + 抓包循环 + 信号
  src/probe.rs          # 能力探测与中文 remediation
  src/install_privileges.rs  # 特权四步（幂等、无参数）
  tests/pcapng_writer.rs     # 独立实现的 pcapng 读取器逐字段校验
  tests/usbmon_decode.rs     # 头解析/方向/长度/setup/填充包/等时跳过/批处理计数/参数校验
  tests/cli_contract.rs      # 运行真实二进制：参数与路径校验、probe、no-interface 不残留 .partial
```

## 测试覆盖

`cargo test --offline --manifest-path apps/usb-capture-helper/Cargo.toml`（20 个测试）：

- **pcapng**：SHB/IDB/EPB 的类型、两侧长度、linktype 220、snaplen、`if_name` 选项编码与 4 字节对齐、
  时间戳高低字顺序（用 > 2^32 的 µs 值）、`cap_len/orig_len`（含短传输）、数据区前 64 字节与载荷一致；
  用测试内**独立实现**的读取器校验，不 import 生产解析代码；写两条事件后 `.partial` → 改名 → 字节数与文件一致。
- **事件解析**：`'S'/'C'/'E'`、控制/批量/中断、IN/OUT、非零 status（含 -32 STALL）、带 setup 的控制提交
  （含 `wValue/wIndex/wLength`）、短传输、`len_cap > 可用字节` 的防御截断、填充包跳过、等时跳过并计数、
  未知 tag/传输类型的跳过计数。
- **参数与路径**：bus（0/非数字/越界/空/负数/小数）、duration-ms、max-bytes 越界、缺值、互斥子命令、
  非 `.pcapng` 后缀、父目录不存在、输出本身/父目录是符号链接——全部返回 `invalidInput`（退出码 2）且不 panic、
  不创建文件（CLI 与库两层都测）。
- **失败路径**：`/dev/usbmon9` 不存在时返回 `no-interface`（退出码 4）且不残留 `.partial`；
  非 root 下调用 `--install-privileges` 立刻返回 `permission`（带 remediation、无任何副作用）。
