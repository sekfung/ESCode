//! MCP 工具绑定投影：把 server 的发现结果变成命名空间化绑定（工具名冲突、schema 校验在这里收口）。
use crate::{
    mcp_config::{self, Server},
    mcp_connection::Connection,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc};

#[derive(Clone)]
pub(super) struct Binding {
    pub(super) name: String,
    pub(super) server: String,
    pub(super) original: String,
    pub(super) key: String,
    pub(super) safe: bool,
    pub(super) definition: Value,
    pub(super) connection: Arc<Connection>,
}

pub(super) fn bind(
    server: &Server,
    key: &str,
    connection: &Arc<Connection>,
    names: &mut BTreeSet<String>,
) -> Result<Vec<Binding>> {
    let mut bindings = vec![];
    for tool in &connection.tools {
        let original = tool["name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .context("MCP tool name required")?;
        let name = mcp_config::tool_name(&server.name, original);
        ensure!(names.insert(name.clone()), "MCP tool namespace collision");
        ensure!(tool["inputSchema"].is_object(), "MCP tool schema required");
        let definition = json!({"type":"function","function":{"name":name,"description":tool["description"].as_str().unwrap_or(""),"parameters":tool["inputSchema"]}});
        ensure!(
            definition.to_string().len() <= 256 * 1024,
            "MCP schema exceeds size limit"
        );
        bindings.push(Binding {
            name,
            server: server.name.clone(),
            original: original.into(),
            key: key.into(),
            safe: tool["annotations"]["readOnlyHint"] == true
                && tool["annotations"]["destructiveHint"] == false,
            definition,
            connection: connection.clone(),
        });
    }
    Ok(bindings)
}
