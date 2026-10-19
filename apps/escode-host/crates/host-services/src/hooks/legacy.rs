//! legacy hooks（TS `fromLegacyHooksConfig`）：`.agents/settings.json` / `.claude/settings.json`
//! 只读；声明顺序（JSON 对象键序）与 matcher/数组顺序保持原样。

use std::path::Path;

use serde_json::{Map, Value, json};

use super::config::{HookEvent, HookType};
use super::ordered_json::{self, OrderedJson};
use super::{digest, paths};

/// legacy `.agents/settings.json` / `.claude/settings.json`（只读，`enabled:false`、`editable:false`）。
pub(super) async fn legacy_hooks(
    source: &'static str,
    scope: &'static str,
    project_path: Option<&Path>,
    path: &Path,
) -> Vec<Value> {
    let Some(root) = tokio::fs::read_to_string(path)
        .await
        .ok()
        .and_then(|text| ordered_json::parse_object(text.as_str()))
    else {
        return Vec::new();
    };
    let Some(hooks) = root.get("hooks").cloned().map(OrderedJson::into_value) else {
        return Vec::new();
    };
    let Value::Object(hooks) = hooks else {
        return Vec::new();
    };
    let directory = paths::parent(path);
    let mut results = Vec::new();
    let mut counter = 0usize;
    for (event_name, matchers) in &hooks {
        let Some(event) = HookEvent::parse(event_name) else {
            continue;
        };
        let Value::Array(matchers) = matchers else {
            continue;
        };
        for matcher in matchers {
            let Value::Object(matcher) = matcher else {
                continue;
            };
            let matcher_value = matcher.get("matcher").cloned();
            let Some(Value::Array(declarations)) = matcher.get("hooks") else {
                continue;
            };
            for declaration in declarations {
                let Value::Object(declaration) = declaration else {
                    continue;
                };
                let kind = match declaration.get("type").and_then(Value::as_str) {
                    Some("command") => HookType::Command,
                    Some("process") => HookType::Process,
                    _ => continue,
                };
                let Some(command) = declaration
                    .get("command")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                else {
                    continue;
                };
                let mut hook = Map::new();
                hook.insert(
                    "id".to_owned(),
                    Value::String(format!("hook-{source}-{scope}-{counter}")),
                );
                counter += 1;
                hook.insert("event".to_owned(), Value::String(event.as_str().to_owned()));
                if let Some(matcher) = &matcher_value {
                    hook.insert("matcher".to_owned(), matcher.clone());
                }
                hook.insert("type".to_owned(), Value::String(kind.as_str().to_owned()));
                hook.insert("command".to_owned(), Value::String(command.to_owned()));
                if kind == HookType::Process {
                    let args = declaration
                        .get("args")
                        .and_then(Value::as_array)
                        .map(|args| {
                            args.iter()
                                .filter_map(Value::as_str)
                                .map(str::to_owned)
                                .map(Value::String)
                                .collect()
                        })
                        .unwrap_or_default();
                    hook.insert("args".to_owned(), Value::Array(args));
                }
                if let Some(async_) = declaration.get("async") {
                    hook.insert("async".to_owned(), async_.clone());
                }
                if let Some(shell) = declaration.get("shell") {
                    hook.insert("shell".to_owned(), shell.clone());
                }
                if let Some(status) = declaration
                    .get("statusMessage")
                    .filter(|value| super::write::is_truthy(value))
                {
                    hook.insert("statusMessage".to_owned(), status.clone());
                }
                let timeout = declaration
                    .get("timeout")
                    .filter(|value| !value.is_null())
                    .cloned()
                    .or_else(|| {
                        declaration
                            .get("timeoutMs")
                            .and_then(Value::as_f64)
                            .map(|value| digest::number((value / 1000.0).round()))
                    });
                if let Some(timeout) = timeout {
                    hook.insert("timeout".to_owned(), timeout);
                }
                hook.insert("enabled".to_owned(), Value::Bool(false));
                hook.insert("editable".to_owned(), Value::Bool(false));
                let mut location = json!({
                    "source": source,
                    "scope": scope,
                    "directoryPath": paths::path_text(&directory),
                });
                if let Some(project_path) = project_path {
                    location["projectPath"] = Value::String(paths::path_text(project_path));
                }
                hook.insert("location".to_owned(), location);
                results.push(Value::Object(hook));
            }
        }
    }
    results
}
