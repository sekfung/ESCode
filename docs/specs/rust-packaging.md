# Rust runtime 打包与选择（WP8）

2026-09-24。此前 Rust runtime 只能通过 `ZCODE_AGENT_SERVER_COMMAND` 指向本地二进制启用；桌面安装包不含 Rust 二进制，`ZCODE_AGENT_SERVER_RUNTIME=zcode-cli-rust` 单独设置时被忽略（仍启动 Node）。**默认 runtime 不变（Node）**。

## 规则

- 描述符（`packages/shared/src/zcode-agent-runtime.ts`，唯一事实源）：新增 `rustBinaryName = "zcode-cli-rust"` 与 `resolveRustBinarySegments(platform)`（Windows 追加 `.exe`），与 Node bundle 同放 `glm/` 资源目录。
- 查找（`providerRuntimeResolver.findZCodeAgentRustBinary`）：候选链与 Node bundle 完全平行（packaged resources → `~/.zcode/server/agents/glm` → `bundled-agents/<platform>/glm` → legacy）。
- 选择（`resolveDefaultZCodeAgentCommand`，唯一 owner）：
  1. `ZCODE_AGENT_SERVER_COMMAND` 显式命令（现状不变）；
  2. 未设命令但 `ZCODE_AGENT_SERVER_RUNTIME=zcode-cli-rust`：使用随包 Rust 二进制，参数 `app-server --stdio --cwd <workspacePath>`，`storagePreparationMode: "process"`、`supportsStorageStartup: true`（与显式命令路径相同）；
  3. 找不到随包 Rust 二进制：**回退 Node**，并以 `warn` 记录 `zcode_agent.runtime.rust_binary_missing`——不静默；
  4. 其余情况维持现有顺序（dev 源码 → Electron Node bundle → 已部署二进制）。
     `ZCODE_AGENT_SERVER_RUNTIME` 接受 `node`（显式回退，与未设置等价）与 `zcode-cli-rust`；其他值抛错（此前只在设置了命令时校验）。
- 构建（`packages/desktop/scripts/prepare-rust-agent.mjs`，由 `prepare-runtime-assets` 在 `ZCODE_BUNDLE_RUST_AGENT=1` 时调用）：按目标平台映射 Rust target triple（`win32-x64 → x86_64-pc-windows-msvc`、`win32-arm64 → aarch64-pc-windows-msvc`、`darwin-* → *-apple-darwin`、`linux-* → *-unknown-linux-gnu`；`ZCODE_RUST_TARGET` 可覆盖，例如本机无 MSVC 时用 GNU），`cargo build --release --locked --target <triple> -p zcode-cli-rust`，复制到 `bundled-agents/<platform>/glm/`。
- macOS 签名：`glm/` 在 electron-builder 中被 `signIgnore`（原内容均为 JS）。Rust 二进制是 Mach-O，必须签名：设置 `ZCODE_RUST_CODESIGN_IDENTITY` 时脚本以 hardened runtime 签名；darwin 目标未设置时拒绝打包（不产出无法公证的包）。

## 回退

- 用户/运维回退：移除 `ZCODE_AGENT_SERVER_RUNTIME` 即回到 Node（见 rust-release-rollback.md）。
- 随包二进制缺失：自动回退 Node 并记录原因。
- 未实现：握手失败后的同次启动自动回退（需要进程管理器在 `initialize` 失败时重试 Node 并上报 lifecycle），另行设计。

## 验收

- 单测：Rust 选择、缺失回退、显式命令优先、非法 runtime 抛错；平台 → triple 映射。
- 本机已验证（2026-09-25）：`ZCODE_RUST_TARGET=x86_64-pc-windows-gnu` 下 `prepare:rust-agent` 产出 release 二进制 `bundled-agents/win32-x64/glm/zcode-cli-rust.exe`（24 MB）；Host 真实解析链（未注入）按规则 2 选中它，并在 App harness 下完成一轮（`zcode-cli-rust-runtime-selection.test.ts`，未随包时该用例跳过）。
- 未验证：MSVC 目标构建、macOS 签名与公证、Linux 包（本机环境限制，如实标注）。
