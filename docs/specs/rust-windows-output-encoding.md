# Rust 工具输出的 Windows 旧代码页解码

2026-10-08。中文 Windows 上 cmd / 旧版本地工具按系统 OEM 代码页（如 936 GBK）输出。TS 用
`adapters/src/exec/outputEncoding.ts` 识别并解码，Rust 一律 `String::from_utf8_lossy`，模型与 UI 看到乱码。
发现于 `escode-cli-rust-shell-resume.test.ts`「changing the shell setting applies to an existing session」：`ver` 的
输出 Node 为 `Microsoft Windows [版本 …]`，Rust 为乱码（GitHub CI 是英文 Windows，测不到）。

## 规则（对齐 TS）

1. 旧编码只在 Windows 上存在；其他平台始终按 UTF-8。
2. 旧编码的解析顺序（TS `resolveLegacyExecutionOutputEncoding`）：
   - `ESCODE_WINDOWS_OUTPUT_ENCODING` 非空：可识别即用，不可识别即「无旧编码」（TS `iconv.encodingExists` 失败回 null）；
   - 系统活动代码页（TS 在 cmd 里跑 `chcp`；Rust 用 `GetOEMCP`，无控制台子进程里两者相同）不是 65001 且有效时用它；
   - 否则按 locale 推断：`LC_ALL` / `LC_CTYPE` / `LANG` 与用户默认 locale 名拼接后小写，子串匹配
     `zh|chinese|cn|hans|hant` → GB18030（54936），`ja|japanese|jp` → 932，`ko|korean|kr` → 949，
     `ru|russian` → 866，其余 437。
3. 解码（TS `decodeExecutionOutputBuffer`）：整段是合法 UTF-8 → UTF-8；否则有旧编码时按旧编码整段解码，没有时按
   UTF-8 lossy。
4. 应用点（三处读输出文件 + 自定义命令的 shell 展开，均为整段解码，对应 TS `BashFileOutput` / `OutputCollector`）：
   前台 Bash 的输出头、TaskOutput、后台详情输出尾窗、自定义命令 `!` 展开。

## 所有者与时序

- 所有者：`escode_cli_host::output_encoding`。旧编码在进程内首次需要时解析一次（`OnceLock`）。TS 每次执行都同步跑一次
  `chcp`（1s 超时）；代码页与该环境变量在进程生命周期内不变，缓存不改变结果，也省掉每次执行的子进程。
- 代码页解码走 `MultiByteToWideChar`（`windows-sys` 的 `Win32_Globalization`），支持系统安装的全部代码页（含
  437/850 等 OEM 页），不新增 crate。

## 已知差异

- 末尾被截断的 UTF-8 多字节序列：TS 把「合法但不完整」也判为非 UTF-8，整段改按旧编码解码——前台输出头（固定字节数截取）
  恰好切在中文字符中间时，Windows 上整段变乱码。Rust 只对**真正非法**的 UTF-8 改用旧编码，截断尾部按 lossy 处理。
  这是有意偏离（TS 的行为是缺陷），只影响 Windows 上被截断的 UTF-8 输出。
- 用户在 cmd 的 `AutoRun` 里改了 `chcp`：TS 读到改后的代码页，Rust 读 OEM 代码页。极少见，不处理。

## 验收

- 单测（host）：合法 UTF-8 原样；GBK 字节在 936 下解码为中文；截断的 UTF-8 尾部不切换旧编码；环境变量覆盖的识别
  （`cp936` / `936` / `gbk` / `utf-8` / 不可识别）；locale 推断表。
- App 差分：`escode-cli-rust-shell-resume.test.ts`「changing the shell setting applies to an existing session on both
  runtimes」在中文 Windows 上两侧 `ver` 输出一致。
