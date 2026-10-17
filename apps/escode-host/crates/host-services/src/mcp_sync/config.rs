//! MCP 配置文件的读写与存量 `enable` 迁移（TS `mcpSyncService.ts` 的 JSON 部分）。
//!
//! 错误文案与 TS 一致（用户可见）：读取失败、解析失败、根不是对象。
//! 写盘走原子写且临时文件 0600（配置含 env/header/token 等 secret）。

use std::path::Path;

use serde_json::{Map, Value};

use super::paths::ConfigKey;

pub type JsonObject = Map<String, Value>;

/// TS `readJsonObject`：ENOENT → None；其它读取/解析错误带文件名上抛。
pub fn read_json_object(path: &Path) -> Result<Option<JsonObject>, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!("无法读取 MCP 配置文件 {}: {error}", path.display()));
        }
    };
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|error| format!("无法解析 MCP 配置文件 {}: {error}", path.display()))?;
    let Value::Object(object) = parsed else {
        return Err(format!("MCP 配置文件 {} 必须是 JSON 对象", path.display()));
    };
    Ok(Some(object))
}

/// TS `writeTextAtomic` + `JSON.stringify(next, null, 2) + "\n"`。
pub fn write_json(path: &Path, object: &JsonObject) -> Result<(), String> {
    let mut text = serde_json::to_string_pretty(object).map_err(|error| error.to_string())?;
    text.push('\n');
    escode_host_core::atomic_write::write_secret(path, text.as_bytes())
        .map_err(|error| format!("{error:#}"))
}

pub fn read_server_map(object: &JsonObject, key: ConfigKey) -> Map<String, Value> {
    let servers = match key {
        ConfigKey::McpServers => object
            .get("mcp")
            .and_then(Value::as_object)
            .and_then(|mcp| mcp.get("servers")),
        ConfigKey::Plain => object.get("mcpServers"),
    };
    servers
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

pub fn write_server_map(
    current: &JsonObject,
    key: ConfigKey,
    servers: Map<String, Value>,
) -> JsonObject {
    let mut next = current.clone();
    match key {
        ConfigKey::Plain => {
            next.insert("mcpServers".to_owned(), Value::Object(servers));
        }
        ConfigKey::McpServers => {
            let mut mcp = current
                .get("mcp")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            mcp.insert("servers".to_owned(), Value::Object(servers));
            next.insert("mcp".to_owned(), Value::Object(mcp));
        }
    }
    next
}

/// TS `readServerEnabled`：只有显式 `enabled: false` 才算停用。
pub fn enabled(config: &Value) -> bool {
    !matches!(config.get("enabled"), Some(Value::Bool(false)))
}

/// TS `setServerEnabled`：去掉 legacy `enable` 与 `enabled`；启用态不落冗余字段。
pub fn set_enabled(config: &Value, enabled: bool) -> Value {
    let mut next = config.as_object().cloned().unwrap_or_default();
    next.remove("enable");
    next.remove("enabled");
    if !enabled {
        next.insert("enabled".to_owned(), Value::Bool(false));
    }
    Value::Object(next)
}

/// TS `migrateLegacyEnableFlag`：冲突时以「停用」为准；只在出现 `enable` 时改写。
pub fn migrate_legacy(servers: Map<String, Value>) -> (Map<String, Value>, bool) {
    let mut changed = false;
    let mut migrated = Map::new();
    for (name, config) in servers {
        let Some(object) = config.as_object() else {
            migrated.insert(name, config);
            continue;
        };
        if !object.contains_key("enable") {
            migrated.insert(name, config);
            continue;
        }
        changed = true;
        let disabled = object.get("enable") == Some(&Value::Bool(false))
            || object.get("enabled") == Some(&Value::Bool(false));
        migrated.insert(name, set_enabled(&config, !disabled));
    }
    (migrated, changed)
}

/// TS `readServerMapWithLegacyMigration`：迁移后落盘；写盘失败不阻断读取。
pub fn read_server_map_with_migration(
    path: &Path,
    object: &JsonObject,
    key: ConfigKey,
) -> Map<String, Value> {
    let (servers, changed) = migrate_legacy(read_server_map(object, key));
    if changed {
        let next = write_server_map(object, key, servers.clone());
        let _ = write_json(path, &next);
    }
    servers
}

/// TS `removeLegacyMcpEnabledOverride`：删掉 `mcp.<目录路径>.<name>`。
pub fn remove_legacy_override(object: &mut JsonObject, directory_path: &str, name: &str) -> bool {
    let Some(mcp) = object.get_mut("mcp").and_then(Value::as_object_mut) else {
        return false;
    };
    let Some(per_directory) = mcp.get_mut(directory_path).and_then(Value::as_object_mut) else {
        return false;
    };
    if per_directory.remove(name).is_none() {
        return false;
    }
    if per_directory.is_empty() {
        mcp.remove(directory_path);
    }
    true
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
