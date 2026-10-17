//! MCP 目录记录的读取、`loadMcpFromUserDirectory` 与 `saveMcpToUserDirectory`。

use std::path::PathBuf;

use serde_json::{Map, Value, json};

use super::config::{self, JsonObject, enabled, read_json_object, write_json, write_server_map};
use super::paths::{self, AGENTS, ConfigKey, ESCODE, Scope};
use crate::skills::Compare;

#[derive(Clone)]
pub(super) struct Record {
    pub name: String,
    pub config: Value,
    pub enabled: bool,
    pub source: &'static str,
    pub path: PathBuf,
}

/// TS `collectEffectiveUserMcpRecords`：`.escode` 有任何 server 就用它，否则用 `.agents`。
pub(super) fn effective_user_records(
    home: &std::path::Path,
    compare: &Compare,
) -> Result<Vec<Record>, String> {
    let escode = read_user_records(home, ESCODE)?;
    if !escode.is_empty() {
        return Ok(sorted(escode, compare));
    }
    Ok(sorted(read_user_records(home, AGENTS)?, compare))
}

fn read_user_records(
    home: &std::path::Path,
    descriptor: paths::Descriptor,
) -> Result<Vec<Record>, String> {
    let path = paths::config_path(descriptor, Scope::User, home, None)?;
    let Some(parsed) = read_json_object(&path)? else {
        return Ok(Vec::new());
    };
    records_from_object(descriptor, &path, &parsed)
}

fn records_from_object(
    descriptor: paths::Descriptor,
    path: &std::path::Path,
    parsed: &JsonObject,
) -> Result<Vec<Record>, String> {
    let servers = config::read_server_map_with_migration(path, parsed, descriptor.config_key);
    Ok(servers
        .into_iter()
        .map(|(name, config)| Record {
            name,
            enabled: enabled(&config),
            config,
            source: descriptor.source,
            path: path.to_path_buf(),
        })
        .collect())
}

fn sorted(mut records: Vec<Record>, compare: &Compare) -> Vec<Record> {
    records.sort_by(|left, right| compare(&left.name, &right.name));
    records
}

/// TS `loadMcpFromUserDirectory`：workspace 源在前，user 源在后；每个源各自优先 `.escode`。
pub(super) fn load(home: &std::path::Path, params: &Value) -> Result<Value, String> {
    let workspace = params["workspacePath"]
        .as_str()
        .filter(|path| !path.is_empty());
    let mut servers = Vec::new();
    if let Some(workspace) = workspace {
        servers.extend(directory_records(home, Scope::Workspace, Some(workspace))?);
    }
    servers.extend(directory_records(home, Scope::User, workspace)?);
    Ok(json!({"servers": servers}))
}

fn directory_records(
    home: &std::path::Path,
    scope: Scope,
    workspace: Option<&str>,
) -> Result<Vec<Value>, String> {
    let (descriptor, path, records) = preferred_directory_records(home, scope, workspace)?;
    let location = paths::location(descriptor, scope, home, workspace)?;
    let file = paths::file_reference(&path);
    Ok(records
        .into_iter()
        .map(|record| {
            let mut value = Map::new();
            value.insert("source".to_owned(), Value::from("escodeagentmcp"));
            value.insert("scope".to_owned(), Value::from(scope.as_str()));
            value.insert("name".to_owned(), Value::from(record.name));
            value.insert("config".to_owned(), record.config);
            value.insert("enabled".to_owned(), Value::Bool(record.enabled));
            if scope == Scope::Workspace
                && let Some(workspace) = workspace
            {
                value.insert("projectPath".to_owned(), Value::from(workspace));
            }
            value.insert("location".to_owned(), location.clone());
            value.insert("file".to_owned(), file.clone());
            Value::Object(value)
        })
        .collect())
}

fn preferred_directory_records(
    home: &std::path::Path,
    scope: Scope,
    workspace: Option<&str>,
) -> Result<(paths::Descriptor, PathBuf, Vec<Record>), String> {
    let path = paths::config_path(ESCODE, scope, home, workspace)?;
    let escode = read_directory_records(ESCODE, &path)?;
    if !escode.is_empty() {
        return Ok((ESCODE, path, escode));
    }
    let agents_path = paths::config_path(AGENTS, scope, home, workspace)?;
    let agents = read_directory_records(AGENTS, &agents_path)?;
    Ok((AGENTS, agents_path, agents))
}

fn read_directory_records(
    descriptor: paths::Descriptor,
    path: &std::path::Path,
) -> Result<Vec<Record>, String> {
    let Some(parsed) = read_json_object(path)? else {
        return Ok(Vec::new());
    };
    records_from_object(descriptor, path, &parsed)
}

/// TS `saveMcpToUserDirectory`。
pub(super) fn save(home: &std::path::Path, payload: &Value) -> Result<(), String> {
    let action = payload["action"].as_str().unwrap_or_default();
    if action == "set-enabled" {
        let Some(enabled) = payload.get("enabled").and_then(Value::as_bool) else {
            return Err("Missing enabled value for MCP set-enabled action".to_owned());
        };
        let project_path = payload["projectPath"]
            .as_str()
            .filter(|path| !path.is_empty());
        let scope = if project_path.is_some() {
            Scope::Workspace
        } else {
            Scope::User
        };
        let location = match payload.get("location").and_then(Value::as_object) {
            Some(_) => paths::Location::from_json(&payload["location"]),
            None => paths::Location {
                source: ESCODE.source.to_owned(),
                scope,
                directory_path: paths::config_path(ESCODE, scope, home, project_path)?
                    .parent()
                    .map(|parent| parent.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                project_path: project_path.map(str::to_owned),
            },
        };
        write_server_enabled(
            home,
            &location,
            payload["name"].as_str().unwrap_or_default(),
            enabled,
        )?;
        cleanup_legacy_user_config(
            home,
            &location,
            payload["name"].as_str().unwrap_or_default(),
        )?;
        return Ok(());
    }

    let project_path = payload["projectPath"]
        .as_str()
        .filter(|path| !path.is_empty());
    let scope = if project_path.is_some() {
        Scope::Workspace
    } else {
        Scope::User
    };
    let file = paths::config_path(ESCODE, scope, home, project_path)?;
    let mut servers: Map<String, Value> = preferred_escode_servers(&file)?;
    let name = payload["name"].as_str().unwrap_or_default().to_owned();
    if action == "upsert" {
        let config = payload
            .get("config")
            .filter(|value| !value.is_null())
            .ok_or_else(|| "Missing MCP config for upsert action".to_owned())?;
        servers.insert(name, config.clone());
    } else {
        servers.remove(&name);
    }
    let current = read_json_object(&file)?.unwrap_or_default();
    write_json(
        &file,
        &write_server_map(&current, ESCODE.config_key, servers),
    )
}

fn preferred_escode_servers(file: &std::path::Path) -> Result<Map<String, Value>, String> {
    let Some(parsed) = read_json_object(file)? else {
        return Ok(Map::new());
    };
    Ok(config::read_server_map(&parsed, ConfigKey::McpServers))
}

fn write_server_enabled(
    home: &std::path::Path,
    location: &paths::Location,
    name: &str,
    enabled: bool,
) -> Result<(), String> {
    let descriptor = paths::descriptor_for_source(&location.source)?;
    let scope = location.scope;
    let workspace = if scope == Scope::Workspace {
        location.project_path.as_deref()
    } else {
        None
    };
    let file = paths::config_path(descriptor, scope, home, workspace)?;
    let current = read_json_object(&file)?.unwrap_or_default();
    let servers = config::read_server_map(&current, descriptor.config_key);
    let Some(server) = servers.get(name).cloned() else {
        return Ok(());
    };
    if !server.is_object() {
        return Ok(());
    }
    let mut next_servers = servers;
    next_servers.insert(name.to_owned(), config::set_enabled(&server, enabled));
    let mut next = write_server_map(&current, descriptor.config_key, next_servers);
    config::remove_legacy_override(&mut next, &location.directory_path, name);
    write_json(&file, &next)
}

fn cleanup_legacy_user_config(
    home: &std::path::Path,
    location: &paths::Location,
    name: &str,
) -> Result<(), String> {
    let file = paths::config_path(ESCODE, Scope::User, home, None)?;
    let Some(mut current) = read_json_object(&file)? else {
        return Ok(());
    };
    if config::remove_legacy_override(&mut current, &location.directory_path, name) {
        write_json(&file, &current)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "read_tests.rs"]
mod tests;
