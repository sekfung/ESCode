//! 内置 `circuit` MCP（docs/circuit/README.md §3.5、§5），对齐 TS `resolveBuiltInCircuitMcpServers`：
//! 只有 Host 声明持有 CircuitService（Desktop 本地 / 远程 workspace Host 注入 `ESCODE_HOST_CIRCUIT=1`）且
//! `circuit` 官方插件启用时注册；与 serial 同样经 Host 插件启动器运行插件的 `dist/mcp/server.js`，
//! broker 连接材料由 hub 定向注入。
use super::{extension_plugins::Plugin, mcp_config::Server};
use std::path::Path;

pub(super) const NAME: &str = "circuit";
const PLUGIN: &str = "circuit@zcode-plugins-official";
const HOST_FLAG: &str = "ESCODE_HOST_CIRCUIT";
/// 电路工具都是短请求，留出 Host 往返与 64 KiB 分页响应余量（TS CIRCUIT_MCP_TIMEOUT_MS 同）。
const TIMEOUT_MS: u64 = 60_000;
/// 官方插件 `defaultAllowedTools`（§5「需要批准」为否）：`circuit_export` 不在名单里，导出仍需用户批准。
const ALLOWED_TOOLS: [&str; 7] = [
    "circuit_parse",
    "circuit_submit_recognition",
    "circuit_get",
    "circuit_query",
    "circuit_correct",
    "circuit_check",
    "circuit_compare_firmware",
];

pub(super) fn host_circuit_available() -> bool {
    std::env::var(HOST_FLAG).is_ok_and(|v| v.trim() == "1")
}

pub(super) fn server(plugins: &[Plugin], cwd: &Path) -> Option<Server> {
    if !host_circuit_available() {
        return None;
    }
    super::mcp_serial::host_plugin_server(NAME, PLUGIN, TIMEOUT_MS, plugins, cwd)
}

/// 并入权限 `allowedTools` 的免批准电路工具。只有 Host 声明电路能力时才加入；插件关闭时这些工具不存在，放行项不生效。
pub(super) fn default_allowed_tools(available: bool) -> Vec<String> {
    if !available {
        return vec![];
    }
    ALLOWED_TOOLS.iter().map(|tool| format!("mcp__{NAME}__{tool}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_tools_exclude_export_and_require_host_flag() {
        let allowed = default_allowed_tools(true);
        assert_eq!(allowed.len(), 7);
        assert!(allowed.contains(&"mcp__circuit__circuit_parse".to_owned()));
        assert!(allowed.contains(&"mcp__circuit__circuit_correct".to_owned()));
        assert!(!allowed.iter().any(|tool| tool.ends_with("circuit_export")));
        assert!(default_allowed_tools(false).is_empty());
    }
}
