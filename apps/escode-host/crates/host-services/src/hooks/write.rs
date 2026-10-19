//! `saveHooks`：与 TS `workspaceHookSettingsModel.ts` / `workspace-hook-mutation.ts` 同格式写回。
//!
//! 关键性质：
//! - 顶层与 `hooks` 对象保留旧键与键序（`OrderedJson`），未知键（`features`/`storage`/…）不丢；
//! - 写临时文件（0600）+ fsync + rename，watcher 看不到半截 JSON；
//! - 只写 user/project 两个 ESCode 配置文件；legacy 与其他来源永不写。

use std::path::Path;

use indexmap::IndexMap;
use serde_json::Value;

use super::digest;
use super::ordered_json::{self, OrderedJson};
use super::paths;

pub async fn save(home: &Path, params: &Value) -> Result<(), String> {
    let workspace_text = params
        .get("workspacePath")
        .and_then(Value::as_str)
        .ok_or("saveHooks requires workspacePath")?;
    let workspace = paths::normalize_absolute(workspace_text)?;
    let hooks = params
        .get("hooks")
        .and_then(Value::as_array)
        .ok_or("saveHooks requires a hooks array")?;
    let current_project_config = paths::join(&workspace, &[".escode", "config.json"]);
    let user_hooks: Vec<&Value> = hooks.iter().filter(|hook| is_user_hook(hook)).collect();
    let project_hooks: Vec<&Value> = hooks
        .iter()
        .filter(|hook| is_project_hook(hook, &current_project_config))
        .collect();
    let user_config = paths::join(home, &[".escode", "cli", "config.json"]);
    write_config(&user_config, &user_hooks).await?;
    write_config(&current_project_config, &project_hooks).await
}

fn is_user_hook(hook: &Value) -> bool {
    if hook.get("editable") == Some(&Value::Bool(false)) {
        return false;
    }
    match hook.get("location") {
        None | Some(Value::Null) => true,
        Some(Value::Object(location)) => {
            location.get("source").and_then(Value::as_str) == Some("escode")
                && location.get("scope").and_then(Value::as_str) == Some("user")
        }
        Some(_) => false,
    }
}

fn is_project_hook(hook: &Value, current_project_config: &Path) -> bool {
    if hook.get("editable") == Some(&Value::Bool(false)) {
        return false;
    }
    let Some(Value::Object(location)) = hook.get("location") else {
        return false;
    };
    if location.get("source").and_then(Value::as_str) != Some("escode")
        || location.get("scope").and_then(Value::as_str) != Some("project")
    {
        return false;
    }
    match hook.get("configuredState") {
        None | Some(Value::Null) => true,
        Some(Value::Object(state)) => state
            .get("sourcePath")
            .and_then(Value::as_str)
            .and_then(|path| paths::normalize_absolute(path).ok())
            .is_some_and(|path| path == current_project_config),
        Some(_) => false,
    }
}

async fn write_config(path: &Path, hooks: &[&Value]) -> Result<(), String> {
    let existing = tokio::fs::read_to_string(path)
        .await
        .ok()
        .and_then(|text| ordered_json::parse_object(&text))
        .unwrap_or_default();
    let mut root = existing;
    let mut hooks_object = root
        .get("hooks")
        .and_then(OrderedJson::as_object)
        .cloned()
        .unwrap_or_default();
    let enabled = resolve_next_root_enabled(hooks_object.get("enabled"), hooks);
    if let Some(enabled) = enabled {
        hooks_object.insert("enabled".to_owned(), enabled);
    }
    hooks_object.insert("events".to_owned(), OrderedJson::Object(to_events(hooks)?));
    root.insert("hooks".to_owned(), OrderedJson::Object(hooks_object));
    let text = ordered_json::to_pretty(&OrderedJson::Object(root))
        .map_err(|error| format!("Workspace Hook config serialization failed: {error}"))?;
    escode_host_core::atomic_write::write_secret(path, text.as_bytes())
        .map_err(|error| format!("Workspace Hook config write failed: {error:#}"))
}

fn resolve_next_root_enabled(
    existing: Option<&OrderedJson>,
    hooks: &[&Value],
) -> Option<OrderedJson> {
    let enabled_now = hooks.iter().any(|hook| {
        if hook.get("enabled") != Some(&Value::Bool(true)) {
            return false;
        }
        match hook.get("configuredState").and_then(Value::as_object) {
            None => true,
            Some(state) => state.get("configuredEnabled") != Some(&Value::Bool(true)),
        }
    });
    if enabled_now {
        Some(OrderedJson::Bool(true))
    } else {
        existing.cloned()
    }
}

fn to_events(hooks: &[&Value]) -> Result<IndexMap<String, OrderedJson>, String> {
    struct Group {
        matcher: Option<String>,
        hooks: Vec<OrderedJson>,
    }
    let mut events: IndexMap<String, Vec<Group>> = IndexMap::new();
    for hook in hooks {
        let event = hook
            .get("event")
            .and_then(Value::as_str)
            .ok_or("saveHooks requires hook.event")?;
        let matcher = hook
            .get("matcher")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let groups = events.entry(event.to_owned()).or_default();
        let index = match groups.iter().position(|group| group.matcher == matcher) {
            Some(index) => index,
            None => {
                groups.push(Group {
                    matcher,
                    hooks: Vec::new(),
                });
                groups.len() - 1
            }
        };
        groups[index].hooks.push(writable_hook(hook));
    }
    let mut result = IndexMap::new();
    for (event, groups) in events {
        let matchers = groups
            .into_iter()
            .map(|group| {
                let mut object = IndexMap::new();
                if let Some(matcher) = group.matcher.filter(|value| !value.is_empty()) {
                    object.insert("matcher".to_owned(), OrderedJson::String(matcher));
                }
                object.insert("hooks".to_owned(), OrderedJson::Array(group.hooks));
                OrderedJson::Object(object)
            })
            .collect();
        result.insert(event, OrderedJson::Array(matchers));
    }
    Ok(result)
}

fn writable_hook(hook: &Value) -> OrderedJson {
    let mut object = IndexMap::new();
    if let Some(custom) = hook.get("custom").and_then(Value::as_object) {
        for (key, value) in custom {
            object.insert(key.clone(), OrderedJson::from_value(value));
        }
    }
    for key in ["type", "command"] {
        if let Some(value) = hook.get(key) {
            object.insert(key.to_owned(), OrderedJson::from_value(value));
        }
    }
    if let Some(enabled) = resolve_writable_enabled(hook) {
        object.insert("enabled".to_owned(), OrderedJson::from_value(&enabled));
    }
    if let Some(status) = hook.get("statusMessage").filter(|value| is_truthy(value)) {
        object.insert("statusMessage".to_owned(), OrderedJson::from_value(status));
    }
    if hook.get("type").and_then(Value::as_str) == Some("command") {
        if hook.get("async").is_some_and(is_truthy) {
            object.insert("async".to_owned(), OrderedJson::Bool(true));
        }
        if let Some(shell) = hook.get("shell").filter(|value| is_truthy(value)) {
            object.insert("shell".to_owned(), OrderedJson::from_value(shell));
        }
        if let Some(timeout) = hook.get("timeout").filter(|value| is_truthy(value)) {
            object.insert("timeout".to_owned(), OrderedJson::from_value(timeout));
        }
    } else {
        if let Some(args) = hook
            .get("args")
            .and_then(Value::as_array)
            .filter(|args| !args.is_empty())
        {
            object.insert(
                "args".to_owned(),
                OrderedJson::from_value(&Value::Array(args.clone())),
            );
        }
        if let Some(seconds) = hook
            .get("timeout")
            .filter(|value| is_truthy(value))
            .and_then(Value::as_f64)
        {
            object.insert(
                "timeoutMs".to_owned(),
                OrderedJson::from_value(&digest::number(seconds * 1000.0)),
            );
        }
    }
    OrderedJson::Object(object)
}

fn resolve_writable_enabled(hook: &Value) -> Option<Value> {
    let enabled = hook.get("enabled").cloned();
    let Some(state) = hook.get("configuredState").and_then(Value::as_object) else {
        return enabled;
    };
    let configured_enabled = state.get("configuredEnabled");
    if enabled.as_ref() == configured_enabled {
        state.get("declarationEnabled").cloned()
    } else {
        enabled
    }
}

pub(super) fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|value| value != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}
