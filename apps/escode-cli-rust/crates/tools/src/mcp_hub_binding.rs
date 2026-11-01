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
    /// `tools/list` 的 `_meta` 归一化后的 UI 描述（行级 `display.ui` 的唯一来源）。
    pub(super) ui: Option<Value>,
    /// 卡片 `pluginId`：插件 server 取 `plugin:<id>:<server>` 的 id，其余用 server 名顶替。
    pub(super) plugin_id: String,
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
            ui: crate::domain::mcp_tool_ui::normalize_meta(&tool["_meta"]),
            plugin_id: crate::domain::mcp_tool_ui::plugin_id_for_server(&server.name).to_owned(),
            connection: connection.clone(),
        });
    }
    Ok(bindings)
}
