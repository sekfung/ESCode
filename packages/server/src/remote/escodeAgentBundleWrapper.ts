// 远端 ESCode Agent 只运行 Rust runtime（Node runtime 已于 2026-11-09 退场）。
//
// 部署布局：把 escode-cli-rust 放到 agents/<provider>/，再写一个同名 wrapper —— 就是 resolver
// 期望找到的可执行入口（如 agents/glm/escode-agent）—— 由它直接 exec Rust 二进制。
// 这样 provider runtime resolver 不需要为远端保留 Node/JS 分支。
// 开发态与生产态共用同一份 wrapper 语义。

// 远端 Rust runtime（docs/specs/rust-packaging.md「远程环境使用 Rust」）：位于 agents/<provider>/。
// 远端只支持 POSIX，文件名不带 .exe；与 shared 描述符的 rustBinaryName 保持一致。
export const REMOTE_AGENT_RUST_BINARY_NAME = "escode-cli-rust";

export function buildRemoteAgentBundleWrapper(runtimeResourceDir: string): string {
  return [
    "#!/bin/sh",
    "set -eu",
    `exec "$HOME/.escode/server/agents/${runtimeResourceDir}/${REMOTE_AGENT_RUST_BINARY_NAME}" "$@"`,
    "",
  ].join("\n");
}

export function isRemoteAgentBundleWrapperCurrent(
  content: string,
  runtimeResourceDir: string,
): boolean {
  return content.replace(/\r\n/g, "\n") === buildRemoteAgentBundleWrapper(runtimeResourceDir);
}
