//! plugins / mcpServers 的导入（TS `importPluginsForAgent` / `importMcpServersForAgent`）。

use std::path::Path;

use serde_json::{Map, Value, json};

use super::skills_commands::import_directory;
use super::{Selection, TaskResult, agent_matches, mcp_item, plugin_item};
use crate::skills::Compare;

use super::super::candidates::{
    collect_mcp_candidates, collect_plugin_candidates, config_path, target_root,
};
use super::super::discovery::skip::{
    SkipCaches, mcp_skip, plugin_skip, read_json_object, resolve_lexical,
};

pub fn import_plugins(
    home: &Path,
    workspace: Option<&Path>,
    selection: &Selection,
    compare: &Compare,
) -> TaskResult {
    let candidates = collect_plugin_candidates(home, workspace, compare);
    let mut task = TaskResult::new(&selection.agent, "plugins", "pluginResults");
    let mut caches = SkipCaches::default();
    for candidate in candidates
        .iter()
        .filter(|candidate| agent_matches(candidate.agent, selection))
        .filter(|candidate| selection.source_scope_matches(candidate.source_root_scope))
        .filter(|candidate| selection.selected(&candidate.source_path.to_string_lossy()))
    {
        let target_scope = selection
            .target_scope
            .unwrap_or(candidate.source_root_scope);
        let selected_root = target_root(home, workspace, "plugins", target_scope);
        let selected_config = config_path(home, workspace, target_scope);
        let (Some(selected_root), Some(selected_config)) = (selected_root, selected_config) else {
            task.skipped_item(plugin_item(candidate), "targetExists");
            continue;
        };
        let target_path = selected_root.join(
            candidate
                .source_path
                .file_name()
                .map(std::ffi::OsStr::to_os_string)
                .unwrap_or_default(),
        );
        if let Some(reason) = plugin_skip(
            &target_path,
            &candidate.id,
            Some(&selected_config),
            &mut caches,
        ) {
            task.skipped_item(plugin_item(candidate), reason);
            continue;
        }
        let result = (|| -> Result<(), ()> {
            if let Some(parent) = target_path.parent() {
                std::fs::create_dir_all(parent).map_err(|_| ())?;
            }
            import_directory(&candidate.source_path, &target_path, selection.mode)?;
            add_plugin_dir(&selected_config, &target_path)
        })();
        match result {
            Ok(()) => {
                caches.remember_plugin(&selected_config, &candidate.id);
                task.imported_item(plugin_item(candidate));
            }
            Err(()) => task.failed_item(plugin_item(candidate)),
        }
    }
    task
}

pub fn import_mcp(home: &Path, workspace: Option<&Path>, selection: &Selection) -> TaskResult {
    let candidates = collect_mcp_candidates(home, workspace);
    let mut task = TaskResult::new(&selection.agent, "mcpServers", "mcpServerResults");
    let mut caches = SkipCaches::default();
    for candidate in candidates
        .iter()
        .filter(|candidate| agent_matches(candidate.agent, selection))
        .filter(|candidate| selection.source_scope_matches(candidate.source_root_scope))
        .filter(|candidate| selection.selected(&candidate.source_path))
    {
        let target_scope = selection
            .target_scope
            .unwrap_or(candidate.source_root_scope);
        let Some(selected_config) = config_path(home, workspace, target_scope) else {
            task.skipped_item(mcp_item(candidate), "sameNameExists");
            continue;
        };
        if let Some(reason) = mcp_skip(
            target_scope,
            home,
            workspace,
            &candidate.name_key,
            &mut caches,
        ) {
            task.skipped_item(mcp_item(candidate), reason);
            continue;
        }
        match add_mcp_server(&selected_config, &candidate.name, &candidate.config) {
            Ok(()) => {
                caches.remember_mcp(target_scope, &candidate.name_key);
                task.imported_item(mcp_item(candidate));
            }
            Err(()) => task.failed_item(mcp_item(candidate)),
        }
    }
    task
}

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(object) => object,
        _ => Map::new(),
    }
}

fn write_json(path: &Path, value: &Value) -> Result<(), ()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|_| ())?;
    }
    let text = format!("{}\n", serde_json::to_string_pretty(value).map_err(|_| ())?);
    escode_host_core::atomic_write::write_secret(path, text.as_bytes()).map_err(|_| ())
}

/// TS `addPluginDirToConfig`：按词典序 `resolve` 后去重，写回 `plugins.dirs`。
fn add_plugin_dir(config_path: &Path, plugin_path: &Path) -> Result<(), ()> {
    let mut parsed = object(read_json_object(config_path));
    let mut plugins = object(parsed.remove("plugins").unwrap_or_else(|| json!({})));
    let mut dirs: Vec<String> = plugins
        .get("dirs")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let resolved = resolve_lexical(&plugin_path.to_string_lossy());
    if dirs.iter().any(|item| resolve_lexical(item) == resolved) {
        return Ok(());
    }
    dirs.push(resolved.to_string_lossy().into_owned());
    plugins.insert(
        "dirs".to_owned(),
        Value::Array(dirs.into_iter().map(Value::from).collect()),
    );
    parsed.insert("plugins".to_owned(), Value::Object(plugins));
    write_json(config_path, &Value::Object(parsed))
}

/// TS `addMcpServerToEscodeConfig`：只合并 `mcp.servers`，保留其它字段。
fn add_mcp_server(config_path: &Path, name: &str, config: &Value) -> Result<(), ()> {
    let mut parsed = object(read_json_object(config_path));
    let mut mcp = object(parsed.remove("mcp").unwrap_or_else(|| json!({})));
    let mut servers = object(mcp.remove("servers").unwrap_or_else(|| json!({})));
    servers.insert(name.to_owned(), config.clone());
    mcp.insert("servers".to_owned(), Value::Object(servers));
    parsed.insert("mcp".to_owned(), Value::Object(mcp));
    write_json(config_path, &Value::Object(parsed))
}
