# Rust 阶段 2：`terminal` 域（rust-unified-host.md 子 spec，优先序 4）

> 状态：已交付（T1：pty 核心 + kitty/vscode/windows-terminal profile；T2 见 §6.1）· 2026-10-15
> TS 真相源：`packages/services/src/terminal/{terminalService.ts,terminal.ts,terminalProfile.ts,terminalProfileTypes.ts}`。
> `terminalProfileMacOs.ts` 与 Alacritty 检测见 §6 登记差异（T2 切片）。

## 1. 范围

`ServiceChannels.Terminal = "terminal"` 的 6 个成员全部实现：`create`、`write`、`resize`、`dispose`（方法）与
`onDynamicData(id)`、`onDynamicExit(id)`（动态事件）。

不做：`system/integratedTerminalShells.ts`（属 `system` 域，另立切片）；内存诊断计数器 `registerMemoryDiagnosticsProvider("terminal")`
（Rust Host 尚无诊断面，见 §6）。

## 2. 所有者与状态

- **Host 内唯一的 `TerminalService`**：持有 `id → Terminal` 表和自增 id（TS：`String(nextId++)`，从 `"0"` 起，进程内不复用）。
  终端属于 Host，不属于连接——客户端断连不回收 pty（TS 同；TS 的注释把它列为已知泄漏，Rust 照搬，不私自“修好”）。
- 每个 `Terminal`：pty master（resize）、writer（write）、killer、data/exit 监听者列表。
- **事件语义（照搬 TS `Emitter`）**：
  - 监听者只收到**订阅之后**产生的数据；没有监听者时数据丢弃（不缓冲）。
  - `onDynamicData(id)` / `onDynamicExit(id)` 对不存在的 id 抛 `Terminal not found: <id>`（只记日志，客户端无响应，同 TS 事件订阅失败）。
  - 自然退出：先把剩余数据投递完，再向 exit 监听者投递退出码，随后摘除监听者并从表中删除（此后 `write` 等抛 `Terminal not found`）。
  - `dispose`：kill + 摘除监听者 + 删表；**不**再投递 exit（TS 先 dispose emitter，kill 引发的 onExit 无人接收）；对不存在的 id 静默成功。
- **背压（Rust 比 TS 更严格，结果不变）**：pty 读线程 → 有界通道 → 异步 pump → 每监听者有界通道。慢消费者让 pty 读阻塞（内核缓冲满后
  子进程写阻塞），不无界堆内存；无监听者时 pump 直接丢弃。dispose 取消 pump，解除阻塞。

## 3. 启动参数解析（TS `resolveTerminalShell/Cwd/Env`）

| 项 | 规则 |
| --- | --- |
| shell（Unix） | 依次 `SHELL`、`/bin/zsh`、`/bin/bash`、`/bin/sh`，首个可执行者；都没有 → `No usable shell found for terminal startup` |
| shell（Windows） | `pwsh.exe`、`powershell.exe`、`ComSpec`、`cmd.exe`（PATH 查找）；无 → `No usable Windows shell found for terminal startup` |
| cwd | 依次 传入值、`HOME`、`/`，首个存在的目录；无 → `No usable working directory found for terminal startup` |
| env | Host 进程 env（经 `core_api::env` 唯一出口），再按下列规则改写 |

env 改写：
1. macOS：`PATH = merge(PATH, /opt/homebrew/bin, /opt/homebrew/sbin, /usr/local/bin, /usr/local/sbin, /usr/bin, /bin, /usr/sbin, /sbin)`
   （按 `:` 拆分、trim、去空、去重，保序）。
2. `TERM=xterm-256color`；`COLORTERM` 取 trim 后的原值，空则 `truecolor`；`CI=1` 且**原** env 的 `TERM=dumb` 时删除 `CI`。
3. 回退 UTF-8 locale：`LC_ALL`/`LC_CTYPE`/`LANG` 中首个含 `utf-?8`（不区分大小写）者；没有则 macOS `en_US.UTF-8`，其余 `C.UTF-8`。
   `LANG`、`LC_CTYPE` 缺失或（trim 后大写）为 `""`/`C`/`POSIX` 时置为回退值；`LC_ALL` 仅在**已设置**且属上述情形时置为回退值。

`create` 返回 `{id, shell, fontFamily, fontSize?, theme?, fontFamilySource, windowsPty?}`，值为 `undefined` 的字段整键省略。

## 4. 字体 / 主题 profile（TS `resolveTerminalFontProfile`）

输入：settings 的 `terminalFontFamily`、`terminalInheritSystemProfile`（`!== false` 即继承，settings 读失败按继承）与 Host env。

- 自定义字体（trim 后非空）：`source="custom"`，`fontFamily = dedupe(custom)`，`fontSize/theme` 取检测结果（若继承）。
- 否则有检测结果：`source="system"`，`fontFamily = dedupe(detected.fontFamily ?? "ui-monospace")`。
- 否则：`source="fallback"`，`fontFamily` = 固定 12 项回退栈，无 `fontSize/theme`。
- `dedupe(primary)`：按 `,` 拆分、trim、去空，再把回退栈中尚未出现的项依次追加，用 `", "` 连接。
- 检测器顺序（先命中者胜，检测结果须含 fontFamily/fontSize/theme 之一）：`windows-terminal`（仅 win32）→ `vscode` → `kitty`
  （darwin/linux/*bsd）。JSONC 解析：先原样 `JSON.parse`，失败则去注释 + 去尾逗号再解析，结果须是对象。

`windowsPty`：仅 Windows，`{backend:"conpty", buildNumber?}`，buildNumber 为 OS release 第三段整数。

## 5. 验收

1. **TS oracle 差分**：`packages/desktop/test/rustHostTerminal.test.ts` 用同一个脚本化 `SHELL`（读一行、回显、按输入退出）分别驱动真实 TS
   `createTerminalService()`（node-pty）与 Rust Host 的 `terminal` channel（真实 `@escode/client`），断言：
   `create` 返回形状（含 kitty/vscode/自定义字体/回退四种 profile）、数据流内容、resize 生效（脚本打印 `stty size`）、退出码、
   退出后 `write` 的错误文案、`dispose` 后不再收到 exit、未知 id 的错误文案、`COLORTERM`/`TERM`/locale 对子进程可见。
2. **Rust 单测**：env 改写、locale 判定、`dedupe`、JSONC 解析、profile 选择、增量 UTF-8 解码（跨块多字节、非法序列）。
3. **负向验证**：去掉增量解码（逐块 lossy）、让 dispose 也投递 exit、把 profile 回退栈换序，差分/单测必须失败。

## 6. 登记差异

1. **T2 切片未做的检测器**：Alacritty（TOML/YAML）与 macOS（Terminal.app / iTerm2，经 `plutil`）。命中这些终端配置的用户在 Rust Host 下
   得到更低优先级的结果（通常 `fallback`）。macOS 检测无法在 Linux CI 做差分，须在 macOS 机器上验证后再合入。
2. **homedir 回退**：TS cwd 候选含 `os.homedir()`；Rust 只用 `HOME`（`HOME` 缺失且传入 cwd 失效时才有差别）。
3. **diagnostics**：无 `terminal.open` 内存诊断计数。
4. **启动失败文案**：`Failed to start terminal with shell '<shell>' in '<cwd>': <底层错误>` 前缀一致，底层错误文案来自 pty 库而非 node-pty。
5. **退出码**：被信号终止时 node-pty 报 `exitCode=0`（信号另给）；Rust 对齐为 0。
6. **Windows**：代码按 ConPTY（`portable-pty`）实现，`buildNumber` 经 `ver` 解析；**未在 Windows 上编译或运行验证**。
7. **进程退出收口**：Host 退出时不逐个 kill；Unix 上 master 关闭使子 shell 收到 SIGHUP，ConPTY 关闭同样终止附着进程。

## 7. 交付记录

| 项 | 证据 |
| --- | --- |
| 服务实现 | `host-services/src/terminal/`（`mod`/`launch`/`profile`/`jsonc`/`pump`/`stream`）；依赖 `portable-pty 0.9` |
| RPC 面 | `host-rpc/src/domains_terminal.rs`；`host-proto` `IMPLEMENTED` 登记 6 个成员（含 2 个动态事件） |
| Rust 单测 | env/locale/PATH、shell/cwd 回退、kitty 正则（期望值取自 Node 实测）、JSONC、profile 优先级、增量 UTF-8、Windows build 解析（13 例 + 解码 2 例） |
| TS 差分 | `packages/desktop/test/rustHostTerminal.test.ts`：同一脚本化 SHELL 驱动 node-pty 与 Rust；`create` 形状、转录（含回显、中文 emoji、`COLORTERM`/`TERM`/`LANG`/`PWD`）、resize（`stty size`）、退出码、退出后 write/resize/dispose、dispose 后无 exit、失效 cwd 回退；自定义字体与 TS 全等；不继承系统 profile 回退 |
| 负向验证 | ① resize 改成空操作 → 差分失败；② `COLORTERM` 不 trim → 差分失败；③ 解码改回逐块 lossy → 单测失败 |

实测事实（node-pty）：`kill -9`/`kill -TERM` 的退出事件为 `{exitCode:0, signal:9|15}`，`exit 7` 为 `{exitCode:7}`；Rust 对齐为 0（§6.5）。
