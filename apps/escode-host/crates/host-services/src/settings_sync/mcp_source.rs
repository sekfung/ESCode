//! 外部 MCP 配置的读取与格式归一（TS `readMcpServersFromSourceFile`）。
//!
//! 三种格式：`mcpServersJson`（顶层 `mcpServers`）、`mcpJson`（OpenCode 顶层 `mcp`，
//! `command` 数组与 `local/remote` 需要归一）、`codexToml`（TOML 的 `mcp_servers`/`mcpServers`）。

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value};

use super::sources::McpFormat;
use crate::git::paths::js_trim;

pub fn read_source_servers(path: &Path, format: McpFormat) -> BTreeMap<String, Value> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    match format {
        McpFormat::CodexToml => {
            let Ok(parsed) = toml::from_str::<toml::Value>(&raw) else {
                return BTreeMap::new();
            };
            let Ok(json) = serde_json::to_value(parsed) else {
                return BTreeMap::new();
            };
            let servers = json.get("mcp_servers").or_else(|| json.get("mcpServers"));
            normalize_server_map(servers)
        }
        McpFormat::McpJson => {
            let Ok(Value::Object(parsed)) = serde_json::from_str::<Value>(&raw) else {
                return BTreeMap::new();
            };
            normalize_server_map(parsed.get("mcp"))
                .into_iter()
                .map(|(name, config)| (name, normalize_open_code_config(config)))
                .collect()
        }
        McpFormat::McpServersJson => {
            let Ok(Value::Object(parsed)) = serde_json::from_str::<Value>(&raw) else {
                return BTreeMap::new();
            };
            normalize_server_map(parsed.get("mcpServers"))
        }
    }
}

/// TS `normalizeMcpServerMap`：名称为空或配置不是对象时丢弃。
fn normalize_server_map(value: Option<&Value>) -> BTreeMap<String, Value> {
    let mut servers = BTreeMap::new();
    let Some(object) = value.and_then(Value::as_object) else {
        return servers;
    };
    for (name, config) in object {
        if js_trim(name).is_empty() || !config.is_object() {
            continue;
        }
        servers.insert(name.clone(), config.clone());
    }
    servers
}

/// TS `normalizeOpenCodeMcpConfig`：`command: [cmd, ...args]` 与 `type: local|remote` 归一。
fn normalize_open_code_config(config: Value) -> Value {
    let Some(object) = config.as_object() else {
        return config;
    };
    if let Some(Value::Array(items)) = object.get("command") {
        let mut strings = items.iter().filter_map(Value::as_str);
        if let Some(first) = strings.next().filter(|first| !first.is_empty()) {
            let args = strings.map(Value::from).collect::<Vec<_>>();
            let mut rest = object.clone();
            rest.remove("command");
            let kind = rest.remove("type").and_then(|value| {
                value
                    .as_str()
                    .map(|kind| match kind {
                        "local" => "stdio",
                        "remote" => "http",
                        _ => "",
                    })
                    .filter(|kind| !kind.is_empty())
                    .map(Value::from)
            });
            rest.insert("command".to_owned(), Value::from(first));
            rest.insert("args".to_owned(), Value::Array(args));
            if let Some(kind) = kind {
                rest.insert("type".to_owned(), kind);
            }
            return Value::Object(rest);
        }
    }
    if object.get("type").and_then(Value::as_str) == Some("local")
        && let Some(command) = object.get("command").and_then(Value::as_str)
        && !js_trim(command).is_empty()
    {
        let mut next = object.clone();
        next.insert("type".to_owned(), Value::from("stdio"));
        next.insert("command".to_owned(), Value::from(js_trim(command)));
        return Value::Object(next);
    }
    config
}

/// TS `stripExternalMcpTimeoutFields`：导入前丢弃外部格式的等待字段。
pub fn strip_timeout_fields(config: &Value) -> Value {
    let Some(object) = config.as_object() else {
        return config.clone();
    };
    let mut next: Map<String, Value> = object.clone();
    next.remove("timeout");
    next.remove("startup_timeout_sec");
    Value::Object(next)
}

#[cfg(test)]
#[path = "mcp_source_tests.rs"]
mod tests;
