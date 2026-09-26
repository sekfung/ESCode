//! 内置 `node_repl` MCP（docs/specs/rust-browser-use.md 第 1 期），对齐 TS `resolveBuiltInNodeReplMcpServers`：
//! Browser Use 或 Computer Use 启用且存在 `node-repl-host` 宿主时注册；宿主经 Host 提供的插件启动器运行
//! （`ZCODE_PLUGIN_HOST_EXEC_PATH` + `ZCODE_PLUGIN_HOST_ENTRYPOINT` + `__zcode-plugin-host`），缺少即不注册。
use super::{extension_plugins::Plugin, mcp_config::Server};
use serde_json::{Value, json};
use std::path::Path;

pub(super) const NAME: &str = "node_repl";
const BROWSER_USE: &str = "browser-use@zcode-plugins-official";
const COMPUTER_USE: &str = "computer-use@zcode-plugins-official";
const HOST: &str = "node-repl-host@zcode-plugins-official";
const HOST_COMMAND: &str = "__zcode-plugin-host";

pub(super) fn server(plugins: &[Plugin], cwd: &Path) -> Option<Server> {
    let root = |id: &str| plugins.iter().find(|p| p.id == id).map(|p| p.root.clone());
    let (browser, cua) = (root(BROWSER_USE), root(COMPUTER_USE));
    if browser.is_none() && cua.is_none() {
        return None;
    }
    let host = root(HOST)?;
    let launcher = |key: &str| std::env::var(key).ok().filter(|v| !v.trim().is_empty());
    let (exec, entrypoint) = (launcher("ZCODE_PLUGIN_HOST_EXEC_PATH")?, launcher("ZCODE_PLUGIN_HOST_ENTRYPOINT")?);
    // 桌面打包态启动器是 ZCode Helper；缺少 Node 模式会误进 Electron main。
    let mut env = json!({"ELECTRON_RUN_AS_NODE": "1"});
    if let Some(browser) = browser {
        env["ZCODE_PLUGIN_ROOT"] = Value::from(browser.to_string_lossy().into_owned());
    }
    if let Some(cua) = cua {
        env["ZCODE_CUA_PLUGIN_ROOT"] = Value::from(cua.to_string_lossy().into_owned());
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
