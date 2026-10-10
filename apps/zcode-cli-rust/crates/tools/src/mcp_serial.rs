//! 内置 `serial` MCP（docs/specs/serial-agent-tools.md），对齐 TS `resolveBuiltInSerialMcpServers`：
//! 只有 Host 声明本机拥有串口会话（Desktop Local Host 注入 `ZCODE_HOST_SERIAL=1`）且 `serial` 官方插件启用时注册；
//! 经 Host 提供的插件启动器运行 serial-plugin 的 `dist/mcp/server.js`，broker 连接材料由 hub 定向注入。
use super::{extension_plugins::Plugin, mcp_config::Server};
use serde_json::{Value, json};
use std::path::Path;

pub(super) const NAME: &str = "serial";
const PLUGIN: &str = "serial@zcode-plugins-official";
const HOST_FLAG: &str = "ZCODE_HOST_SERIAL";
const HOST_COMMAND: &str = "__zcode-plugin-host";
/// wait_for 上限 120s，加上 Host 往返余量（TS SERIAL_MCP_TIMEOUT_MS 同）。
const TIMEOUT_MS: u64 = 150_000;
/// 官方插件 `defaultAllowedTools`：只读工具免审批；写类工具（open/write/close）走审批。
const READ_ONLY_TOOLS: [&str; 3] = ["serial_list", "serial_read", "serial_wait_for"];

pub(super) fn host_serial_available() -> bool {
    std::env::var(HOST_FLAG).is_ok_and(|v| v.trim() == "1")
}

pub(super) fn server(plugins: &[Plugin], cwd: &Path) -> Option<Server> {
    if !host_serial_available() {
        return None;
    }
    let root = plugins.iter().find(|p| p.id == PLUGIN)?.root.clone();
    let launcher = |key: &str| std::env::var(key).ok().filter(|v| !v.trim().is_empty());
    let (exec, entrypoint) = (launcher("ZCODE_PLUGIN_HOST_EXEC_PATH")?, launcher("ZCODE_PLUGIN_HOST_ENTRYPOINT")?);
    Server::parse(NAME, config(&root, cwd, &exec, &entrypoint), cwd).ok()
}

fn config(root: &Path, cwd: &Path, exec: &str, entrypoint: &str) -> Value {
    let script = root.join("dist").join("mcp").join("server.js");
    json!({
        "type": "stdio",
        "command": exec,
        "args": [entrypoint, HOST_COMMAND, script.to_string_lossy()],
        "cwd": cwd.to_string_lossy(),
        // 桌面打包态启动器是 ZCode Helper；缺少 Node 模式会误进 Electron main。
        "env": {"ELECTRON_RUN_AS_NODE": "1", "ZCODE_PLUGIN_ROOT": root.to_string_lossy()},
        "isolation": "workspace",
        // serial server 只接受现代协商（serveStdio legacy: "reject"），与 node_repl 一样固定协议版本。
        "protocolVersion": "2026-07-28",
        "timeoutMs": TIMEOUT_MS,
    })
}

/// 并入权限 `allowedTools` 的串口只读工具。只有 Host 声明串口能力时才加入；插件关闭时这些工具不存在，放行项不生效。
pub(super) fn default_allowed_tools(available: bool) -> Vec<String> {
    if !available {
        return vec![];
    }
    READ_ONLY_TOOLS.iter().map(|tool| format!("mcp__{NAME}__{tool}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_runs_plugin_server_through_host_launcher() {
        let config = config(Path::new("/plugins/serial"), Path::new("/work"), "/bin/zcode-helper", "/app/zcode.cjs");
        assert_eq!(config["command"], "/bin/zcode-helper");
        assert_eq!(config["args"][0], "/app/zcode.cjs");
        assert_eq!(config["args"][1], HOST_COMMAND);
        assert!(config["args"][2].as_str().unwrap().ends_with("server.js"));
        assert_eq!(config["env"]["ELECTRON_RUN_AS_NODE"], "1");
        assert_eq!(config["isolation"], "workspace");
        assert_eq!(config["protocolVersion"], "2026-07-28");
        assert_eq!(config["timeoutMs"], TIMEOUT_MS);
    }

    #[test]
    fn read_only_tools_are_allowed_only_when_host_declares_serial() {
        assert_eq!(
            default_allowed_tools(true),
            ["mcp__serial__serial_list", "mcp__serial__serial_read", "mcp__serial__serial_wait_for"],
        );
        assert!(default_allowed_tools(false).is_empty());
    }
}
