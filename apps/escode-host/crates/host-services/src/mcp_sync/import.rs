//! `importMcpServers`（TS `mcpSyncService.ts`）：把远端导出的 server 写入用户级 `.escode/cli/config.json`。
//!
//! 判重顺序：目标文件同名 → 用户级两个根已存在同名（trim+lowercase）→ 写入。
//! 写入时只改 `mcp.servers`，保留配置文件其余字段。

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};

use super::config::{self, read_json_object, write_json, write_server_map};
use super::paths::{ESCODE, Scope};
use super::read::{self, Record};
use super::rewrite::{RewritePaths, rewrite_filesystem_mcp_config};
use super::service::name_key;
use crate::skills::Compare;

pub(super) fn import_servers(
    home: &Path,
    compare: &Compare,
    params: &Value,
) -> Result<Value, String> {
    let target = super::paths::config_path(ESCODE, Scope::User, home, None)?;
    let current = read_json_object(&target)?.unwrap_or_default();
    let mut target_servers = config::read_server_map(&current, ESCODE.config_key);
    let mut existing: BTreeMap<String, Record> = BTreeMap::new();
    for record in read::effective_user_records(home, compare)? {
        existing.entry(name_key(&record.name)).or_insert(record);
    }

    let remote_home = home.to_string_lossy().into_owned();
    let rewrite = RewritePaths {
        local_home: params["localHomeDir"].as_str().unwrap_or_default(),
        local_workspace: params["localWorkspacePath"].as_str(),
        remote_home: &remote_home,
        remote_workspace: params["remoteWorkspacePath"].as_str(),
    };
    let mut results = Vec::new();
    let mut changed = false;
    for server in params["servers"].as_array().into_iter().flatten() {
        let name = server["name"].as_str().unwrap_or_default();
        if target_servers.contains_key(name) {
            results.push(json!({
                "name": name, "status": "skipped",
                "path": target.to_string_lossy(),
            }));
            continue;
        }
        let key = name_key(name);
        if let Some(record) = existing.get(&key) {
            results.push(json!({
                "name": name, "status": "skipped",
                "path": record.path.to_string_lossy(),
            }));
            continue;
        }
        let config = &server["config"];
        if !config.is_object() {
            results.push(json!({
                "name": name, "status": "failed",
                "path": target.to_string_lossy(),
                "error": "invalid MCP server config",
            }));
            continue;
        }
        let enabled = js_truthy(&server["enabled"]);
        let rewritten =
            rewrite_filesystem_mcp_config(name, &config::set_enabled(config, enabled), &rewrite);
        target_servers.insert(name.to_owned(), rewritten.clone());
        existing.insert(
            key,
            Record {
                name: name.to_owned(),
                config: rewritten,
                enabled,
                source: ESCODE.source,
                path: target.clone(),
            },
        );
        results.push(json!({
            "name": name, "status": "synced",
            "path": target.to_string_lossy(),
        }));
        changed = true;
    }
    if changed {
        write_json(
            &target,
            &write_server_map(&current, ESCODE.config_key, target_servers),
        )?;
    }
    Ok(json!({"results": results}))
}

/// TS 的 `if (server.enabled)` 真值判断；缺省（undefined）视为 false。
fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|value| value != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}
