//! 用户 CLI 配置与命令启用态覆盖表（TS `commandsService.ts` 的 `readUserCliConfig` /
//! `writeUserCliConfig` / `setCommandEnabledOverride` 一族）。
//!
//! 文件：`<home>/.escode/cli/config.json`。覆盖表放在 `config.command[<文件绝对路径>].enable`，
//! **只存 `false`**（启用即删键，保持文件精简）；`command` 空了连键一起删。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

pub fn user_cli_config_path(home: &Path) -> PathBuf {
    home.join(".escode").join("cli").join("config.json")
}

/// TS `readUserCliConfig`：读不到、坏 JSON、非对象一律回空对象（不抛）。
pub async fn read_config(home: &Path) -> Value {
    let Ok(raw) = tokio::fs::read_to_string(user_cli_config_path(home)).await else {
        return Value::Object(Map::new());
    };
    match serde_json::from_str::<Value>(&raw) {
        Ok(value) if value.is_object() => value,
        _ => Value::Object(Map::new()),
    }
}

/// TS `writeUserCliConfig`：`JSON.stringify(config, null, 2) + "\n"`。
pub async fn write_config(home: &Path, config: &Value) -> Result<(), String> {
    let path = user_cli_config_path(home);
    let mut bytes = serde_json::to_vec_pretty(config).map_err(|error| format!("{error}"))?;
    bytes.push(b'\n');
    tokio::task::spawn_blocking(move || escode_host_core::atomic_write::write(&path, &bytes))
        .await
        .map_err(|error| format!("{error}"))?
        .map_err(|error| format!("{error:#}"))
}

/// TS `readCommandEnabledOverrides`：`config.command` 里每个 `{enable: boolean}` 条目。
pub fn enabled_overrides(config: &Value) -> BTreeMap<String, bool> {
    let mut overrides = BTreeMap::new();
    let Some(command) = config.get("command").and_then(Value::as_object) else {
        return overrides;
    };
    for (file_path, entry) in command {
        if let Some(enabled) = entry.get("enable").and_then(Value::as_bool) {
            overrides.insert(file_path.clone(), enabled);
        }
    }
    overrides
}

/// TS `getCommandEnabled`：没有覆盖即启用（缺省 `true`）。
pub fn is_enabled(overrides: &BTreeMap<String, bool>, file_path: &str) -> bool {
    overrides.get(file_path).copied().unwrap_or(true)
}

/// TS `setCommandEnabledOverride`：启用 → 删键；禁用 → 写 `{enable: false}`；`command` 变空则删键。
pub fn set_override(config: &Value, file_path: &str, enabled: bool) -> Value {
    let mut command = config
        .get("command")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if enabled {
        command.remove(file_path);
    } else {
        let mut entry = Map::new();
        entry.insert("enable".to_owned(), Value::Bool(false));
        command.insert(file_path.to_owned(), Value::Object(entry));
    }

    let mut next = config.as_object().cloned().unwrap_or_default();
    if command.is_empty() {
        next.remove("command");
    } else {
        next.insert("command".to_owned(), Value::Object(command));
    }
    Value::Object(next)
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
