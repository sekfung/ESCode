# Rust Bash 的 shell 初始化快照与 cwd 捕获

## 背景

2026-09-27 Bash 差分发现两处差异：

- 同一条出错命令，Node 报 `line 3`，Rust 报 `line 2`；
- 命令 `cd` 到工作区外时，Node 在结果里追加 `Shell cwd was reset to <root>`，Rust 没有。

原因：

- TS 先把用户 shell 的初始化状态（`.bashrc`/`.zshrc`/`.profile` 里的函数、alias、shell 选项与 PATH）快照成脚本。
  每条命令先 source 这个快照，再以非 login shell 运行，所以脚本多一行；
- Rust 每次都以 login shell（`-l`）运行，不 source 快照；
- TS 在前台 Bash 成功后记录最终的物理 cwd，离开工作区时给出重置提示。

## 规则（对齐 TS `adapters/src/exec/shell-init-snapshot.ts`、`cwd-capture.ts`、`core/.../bash-cwd-policy.ts`）

1. 快照：
   - 只用于 posix 与 git-bash 方言；
   - 按（输出根目录、方言、shell 路径）进程内缓存，每个组合只创建一次；
   - 创建：以 `<shell> -c -l <创建脚本>` 运行，10s 超时，使用工具子进程环境；
   - 创建脚本逐字同 TS：
     - source 用户配置（zsh 为 `~/.zshrc`，bash 为 `~/.bashrc`，其余为 `~/.profile`）；
     - 依次写入 unalias、函数（bash 用 base64 编码）、shell 选项、alias（MSYS/Cygwin 下过滤 winpty）与 PATH；
   - git-bash 的 PATH 取 `<shell> -lc 'echo "$PATH"'`，其余取当前环境的 PATH；
   - 快照文件：`<输出根目录>/shell-snapshots/snapshot-<kind>-<毫秒时间戳>-<6 位随机>.sh`；
   - 创建失败或文件不存在：本次不用快照，照旧以 login shell 运行。
2. 使用：
   - 有快照时，命令脚本第一行为 `. <快照路径> 2>/dev/null || true`，之后才是内置搜索 prelude 的 source 行与命令；
   - shell 以非 login 方式运行（去掉 `-l`）；
   - 每次执行前确认快照文件仍在，不在则不用。
3. 清理：
   - runtime 关闭时删除本进程创建的快照；
   - 启动时删除 `shell-snapshots` 下超过 30 天的 `.sh` 文件。
4. cwd 捕获（前台 Bash；显式后台不做）：
   - 命令后追加：
     ```
     __zcode_status=$?
     if [ "$__zcode_status" -eq 0 ]; then pwd -P > '<cwd 文件>'; fi
     exit "$__zcode_status"
     ```
   - cmd 方言用对应的 `set`/`cd`/`exit /b` 写法；
   - 成功（completed 且退出码 0）并读到 cwd 时：
     - 按物理路径判断是否仍在工作区内（两侧都取 realpath 比较）；
     - 离开工作区时，stderr 追加 `Shell cwd was reset to <工作区根>`。TS 的 `appendBashCwdStderrSuffix`
       先去掉 stderr 末尾的换行，stderr 为空时只有这一行；
   - App 模式下 TS 不保留会话 cwd（差分实测：`cd sub` 后下一次 `pwd` 仍在工作区根），Rust 同样不保留。

## 验收

- 差分（Node vs Rust，同一 fixture）：
  - 出错命令的错误行号一致；
  - `cd` 到工作区外时的提示一致；
  - `pwd` 等输出一致（临时目录按占位比较）。
- 用户 `~/.bashrc` 中定义的函数与 alias 在命令中可用，两侧行为一致（fixture 的 HOME 下写入 `.bashrc`）。

## 实测记录（2026-09-27，Windows 11 本机，Git for Windows）

- TS 自身创建快照（`bash -c -l <脚本>`）约 9.6–9.7s：Git Bash 的 login profile 定义大量函数，逐个 base64 导出。
  这贴着 10s 超时，Node 与 Rust 都会随机成功或回落为 login shell。回落时：
  - `.bashrc` 中的函数仍可用（Git Bash 的 profile 会 source 它）；
  - alias 不展开；
  - 出错行号少一行。
- 这是 TS 的产品行为（Windows 用户可能经常拿不到快照），Rust 按同一规则处理，未改动超时。
- 差分 `zcode-cli-rust-bash-shell.test.ts` 因此只在 Linux/macOS 比较；本机两侧各跑一次均通过
  （Node 64s，Rust 22s）。
