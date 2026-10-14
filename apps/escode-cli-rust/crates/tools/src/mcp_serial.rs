//! 内置 `serial` MCP（docs/specs/serial-agent-tools.md），对齐 TS `resolveBuiltInSerialMcpServers`：
//! 只有 Host 声明本机拥有串口会话（Desktop Local Host 注入 `ESCODE_HOST_SERIAL=1`）且 `serial` 官方插件启用时注册；
//! 经 Host 提供的插件启动器运行 serial-plugin 的 `dist/mcp/server.js`，broker 连接材料由 hub 定向注入。
use super::{extension_plugins::Plugin, mcp_config::Server};
use serde_json::{Value, json};
use std::path::Path;

pub(super) const NAME: &str = "serial";
const PLUGIN: &str = "serial@zcode-plugins-official";
const HOST_FLAG: &str = "ESCODE_HOST_SERIAL";
const HOST_COMMAND: &str = "__escode-plugin-host";
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
    host_plugin_server(NAME, PLUGIN, TIMEOUT_MS, "workspace", &[], plugins, cwd)
}

/// 宿主注入的官方插件 MCP server（serial / circuit / rea 共用）：插件启用时经 Host 插件启动器运行其
/// `dist/mcp/server.js`。
///
/// `isolation` 是入参而不是常量：serial/circuit 的状态所有者是 Host 服务（一个串口 / 一份电路 IR），
/// 进程可以跨会话复用；rea 在服务端持有 binary session，必须按会话独占（TS 侧同，见
/// docs/specs/rea-plugin.md 的「运行时安放」）。
///
/// `extra_env` 是该 server 特有的注入项（rea 的 compact profile、运行时路径与可选引擎路径）。
pub(super) fn host_plugin_server(
    name: &str,
    plugin: &str,
    timeout_ms: u64,
    isolation: &str,
    extra_env: &[(String, String)],
    plugins: &[Plugin],
    cwd: &Path,
) -> Option<Server> {
    let root = plugins.iter().find(|p| p.id == plugin)?.root.clone();
    let launcher = |key: &str| std::env::var(key).ok().filter(|v| !v.trim().is_empty());
    let (exec, entrypoint) = (
        launcher("ESCODE_PLUGIN_HOST_EXEC_PATH")?,
        launcher("ESCODE_PLUGIN_HOST_ENTRYPOINT")?,
    );
    Server::parse(
        name,
        config(
            &root,
            cwd,
            &exec,
            &entrypoint,
            timeout_ms,
            isolation,
            extra_env,
        ),
        cwd,
    )
    .ok()
}

fn config(
    root: &Path,
    cwd: &Path,
    exec: &str,
    entrypoint: &str,
    timeout_ms: u64,
    isolation: &str,
    extra_env: &[(String, String)],
) -> Value {
    let script = root.join("dist").join("mcp").join("server.js");
    // 桌面打包态启动器是 ESCode Helper；缺少 Node 模式会误进 Electron main。
    let mut env =
        json!({"ELECTRON_RUN_AS_NODE": "1", "ESCODE_PLUGIN_ROOT": root.to_string_lossy()});
    if let Some(map) = env.as_object_mut() {
        for (key, value) in extra_env {
            map.insert(key.clone(), Value::String(value.clone()));
        }
    }
    json!({
        "type": "stdio",
        "command": exec,
        "args": [entrypoint, HOST_COMMAND, script.to_string_lossy()],
        "cwd": cwd.to_string_lossy(),
        "env": env,
        "isolation": isolation,
        // serial / circuit / rea 的 server 只接受现代协商（serveStdio legacy: "reject"），固定协议版本。
        "protocolVersion": "2026-07-28",
        "timeoutMs": timeout_ms,
    })
}

/// 并入权限 `allowedTools` 的串口只读工具。只有 Host 声明串口能力时才加入；插件关闭时这些工具不存在，放行项不生效。
pub(super) fn default_allowed_tools(available: bool) -> Vec<String> {
    if !available {
        return vec![];
    }
    READ_ONLY_TOOLS
        .iter()
        .map(|tool| format!("mcp__{NAME}__{tool}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_runs_plugin_server_through_host_launcher() {
        let config = config(
            Path::new("/plugins/serial"),
            Path::new("/work"),
            "/bin/escode-helper",
            "/app/escode.cjs",
            TIMEOUT_MS,
            "workspace",
            &[("EXTRA".to_string(), "1".to_string())],
        );
        assert_eq!(config["command"], "/bin/escode-helper");
        assert_eq!(config["args"][0], "/app/escode.cjs");
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
            [
                "mcp__serial__serial_list",
                "mcp__serial__serial_read",
                "mcp__serial__serial_wait_for"
            ],
        );
        assert!(default_allowed_tools(false).is_empty());
    }
}
