// 远端 ESCode Agent 以「独立 node + 编译产物 zcode.cjs」的形态运行，而不是各平台内嵌 node 的原生二进制。
// 远端部署时本来就有一份独立 node（用于跑 zcode-server.cjs），agent 复用它执行 zcode.cjs 即可。
//
// 部署布局：把 zcode.cjs 放到 agents/<provider>/zcode.cjs，再写一个同名 wrapper —— 就是 resolver
// 期望找到的可执行入口（如 agents/glm/escode-agent）—— 由它用远端 node 执行 zcode.cjs。
// 这样 provider runtime resolver 不需要区分原生/JS，照旧找 escode-agent 这个可执行文件即可。
// 开发态与生产态共用同一份 wrapper 语义。

export const REMOTE_AGENT_BUNDLE_NAME = "zcode.cjs";
// 远端 Rust runtime（docs/specs/rust-packaging.md「远程环境使用 Rust」）：与 zcode.cjs 同在 agents/<provider>/。
// 远端只支持 POSIX，文件名不带 .exe；与 shared 描述符的 rustBinaryName 保持一致。
export const REMOTE_AGENT_RUST_BINARY_NAME = "escode-cli-rust";

export function buildRemoteAgentBundleWrapper(runtimeResourceDir: string): string {
  return [
    "#!/bin/sh",
    "set -eu",
    'runtime_root="${ESCODE_SERVER_RUNTIME_ROOT:-$HOME/.escode/server}"',
    `exec "$runtime_root/node" "$HOME/.escode/server/agents/${runtimeResourceDir}/${REMOTE_AGENT_BUNDLE_NAME}" "$@"`,
    "",
  ].join("\n");
}

export function isRemoteAgentBundleWrapperCurrent(
  content: string,
  runtimeResourceDir: string,
): boolean {
  return content.replace(/\r\n/g, "\n") === buildRemoteAgentBundleWrapper(runtimeResourceDir);
}
