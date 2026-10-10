//! 内置 `node_repl` MCP（docs/specs/rust-browser-use.md 第 1 期），对齐 TS `resolveBuiltInNodeReplMcpServers`：
//! Browser Use 或 Computer Use 启用且存在 `node-repl-host` 宿主时注册；宿主经 Host 提供的插件启动器运行
//! （`ESCODE_PLUGIN_HOST_EXEC_PATH` + `ESCODE_PLUGIN_HOST_ENTRYPOINT` + `__escode-plugin-host`），缺少即不注册。
use super::{extension_plugins::Plugin, mcp_config::Server};
use serde_json::{Value, json};
use std::path::Path;

pub(super) const NAME: &str = "node_repl";
const BROWSER_USE: &str = "browser-use@escode-plugins-official";
const COMPUTER_USE: &str = "computer-use@escode-plugins-official";
const HOST: &str = "node-repl-host@escode-plugins-official";
const HOST_COMMAND: &str = "__escode-plugin-host";

pub(super) fn server(plugins: &[Plugin], cwd: &Path) -> Option<Server> {
    let root = |id: &str| plugins.iter().find(|p| p.id == id).map(|p| p.root.clone());
    let (browser, cua) = (root(BROWSER_USE), root(COMPUTER_USE));
    if browser.is_none() && cua.is_none() {
        return None;
    }
    let host = root(HOST)?;
    let launcher = |key: &str| std::env::var(key).ok().filter(|v| !v.trim().is_empty());
    let (exec, entrypoint) = (launcher("ESCODE_PLUGIN_HOST_EXEC_PATH")?, launcher("ESCODE_PLUGIN_HOST_ENTRYPOINT")?);
    // 桌面打包态启动器是 ESCode Helper；缺少 Node 模式会误进 Electron main。
    let mut env = json!({"ELECTRON_RUN_AS_NODE": "1"});
    if let Some(browser) = browser {
        env["ESCODE_PLUGIN_ROOT"] = Value::from(browser.to_string_lossy().into_owned());
    }
    if let Some(cua) = cua {
        env["ESCODE_CUA_PLUGIN_ROOT"] = Value::from(cua.to_string_lossy().into_owned());
        // TS injectCuaCredentialsIntoNodeRepl（docs/specs/rust-browser-use.md 第 4 期）：凭据成组才注入，
        // 且只进 node_repl；其他子进程的环境里这些键已由 host::child_env 删除。
        if let Some(credentials) = escode_cli_host::child_env::cua_credentials() {
            use crate::domain::runtime_env as keys;
            env[keys::CUA_SOCKET_KEY] = credentials.socket.into();
            if let Some(marker) = credentials.refresh_marker {
                env[keys::CUA_REFRESH_MARKER_KEY] = marker.into();
            }
            env[keys::CUA_AUTHORITY_KEY] = credentials.authority.into();
            env["ESCODE_CUA_NODE_REPL_HOST"] = "1".into();
            env["ESCODE_PLUGIN_ID"] = COMPUTER_USE.into();
        }
    }
    let script = host.join("dist").join("mcp").join("server.js");
    let raw = json!({
        "type": "stdio",
        "command": exec,
        "args": [entrypoint, HOST_COMMAND, script.to_string_lossy()],
        "cwd": cwd.to_string_lossy(),
        "env": env,
        "isolation": "workspace",
        "protocolVersion": "2026-07-28",
        "timeoutMs": 600_000,
    });
    Server::parse(NAME, raw, cwd).ok()
}
