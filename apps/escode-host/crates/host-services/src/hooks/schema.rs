//! Zod `workspaceHooksConfigSchema` 的严格解析（从 `config.rs` 拆出以守住 400 行上限）。
//!
//! 根/事件/matcher strict；hook 声明 passthrough（未知键作为 `custom` 保留）。

use serde_json::{Map, Value};

use super::config::{HookDefinition, HookEvent, HookType, HooksConfig, Matcher, Shell};

/// `workspaceHooksConfigSchema.parse`（strict 根/事件/matcher；hook passthrough）。
pub fn parse_hooks_config(value: &Value) -> Result<HooksConfig, String> {
    let root = as_object(value).ok_or("hooks must be an object")?;
    reject_unknown(
        root,
        &["enabled", "timeoutMs", "maxOutputBytes", "events"],
        "hooks",
    )?;
    let enabled = optional_bool(root.get("enabled"), "hooks.enabled")?;
    let timeout_ms = optional_positive(root.get("timeoutMs"), "hooks.timeoutMs")?;
    let max_output_bytes = optional_positive(root.get("maxOutputBytes"), "hooks.maxOutputBytes")?;
    let mut events: [Option<Vec<Matcher>>; 7] = Default::default();
    if let Some(raw_events) = root.get("events") {
        let events_object = as_object(raw_events).ok_or("hooks.events must be an object")?;
        for (name, raw_matchers) in events_object {
            let event = HookEvent::parse(name).ok_or_else(|| format!("unknown event {name}"))?;
            let list = raw_matchers
                .as_array()
                .ok_or_else(|| format!("hooks.events.{name} must be an array"))?;
            let mut matchers = Vec::with_capacity(list.len());
            for raw in list {
                matchers.push(parse_matcher(raw)?);
            }
            events[event_index(event)] = Some(matchers);
        }
    }
    Ok(HooksConfig {
        enabled,
        timeout_ms,
        max_output_bytes,
        events,
    })
}

fn parse_matcher(value: &Value) -> Result<Matcher, String> {
    let object = as_object(value).ok_or("matcher must be an object")?;
    reject_unknown(object, &["matcher", "hooks"], "matcher")?;
    let matcher = match object.get("matcher") {
        None => None,
        Some(value) => Some(non_empty_string(value, "matcher.matcher")?),
    };
    let raw_hooks = object
        .get("hooks")
        .and_then(Value::as_array)
        .ok_or("matcher.hooks must be an array")?;
    if raw_hooks.is_empty() {
        return Err("matcher.hooks must not be empty".to_owned());
    }
    let hooks = raw_hooks
        .iter()
        .map(parse_hook_definition)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Matcher { matcher, hooks })
}

const HOOK_KEYS: [&str; 9] = [
    "type",
    "command",
    "args",
    "async",
    "enabled",
    "shell",
    "statusMessage",
    "timeout",
    "timeoutMs",
];

fn parse_hook_definition(value: &Value) -> Result<HookDefinition, String> {
    let object = as_object(value).ok_or("hook declaration must be an object")?;
    let kind = match object.get("type").and_then(Value::as_str) {
        Some("command") => HookType::Command,
        Some("process") => HookType::Process,
        _ => return Err("hook.type must be command or process".to_owned()),
    };
    let command = non_empty_string(
        object.get("command").ok_or("hook.command is required")?,
        "hook.command",
    )?;
    let enabled = optional_bool(object.get("enabled"), "hook.enabled")?;
    let status_message = match object.get("statusMessage") {
        None => None,
        Some(value) => Some(non_empty_string(value, "hook.statusMessage")?),
    };
    let timeout_ms = optional_positive(object.get("timeoutMs"), "hook.timeoutMs")?;
    let mut custom = Map::new();
    for (key, value) in object {
        if !HOOK_KEYS.contains(&key.as_str()) {
            custom.insert(key.clone(), value.clone());
        }
    }
    match kind {
        HookType::Process => {
            let args = match object.get("args") {
                None => None,
                Some(value) => Some(parse_string_array(value, "hook.args")?),
            };
            Ok(HookDefinition {
                kind,
                command,
                enabled,
                args,
                async_: None,
                shell: None,
                timeout: None,
                timeout_ms,
                status_message,
                custom,
            })
        }
        HookType::Command => {
            let async_ = optional_bool(object.get("async"), "hook.async")?;
            let shell = match object.get("shell") {
                None => None,
                Some(Value::Bool(true)) => Some(Shell::Enabled),
                Some(Value::String(text)) if !text.is_empty() => Some(Shell::Command(text.clone())),
                Some(_) => return Err("hook.shell must be true or a non-empty string".to_owned()),
            };
            let timeout = optional_positive(object.get("timeout"), "hook.timeout")?;
            Ok(HookDefinition {
                kind,
                command,
                enabled,
                args: None,
                async_,
                shell,
                timeout,
                timeout_ms,
                status_message,
                custom,
            })
        }
    }
}

pub(crate) fn event_index(event: HookEvent) -> usize {
    HookEvent::ALL
        .iter()
        .position(|candidate| *candidate == event)
        .unwrap_or_default()
}

fn as_object(value: &Value) -> Option<&Map<String, Value>> {
    value.as_object()
}

fn reject_unknown(
    object: &Map<String, Value>,
    allowed: &[&str],
    label: &str,
) -> Result<(), String> {
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!("{label} has unknown key {key}"));
        }
    }
    Ok(())
}

fn optional_bool(value: Option<&Value>, label: &str) -> Result<Option<bool>, String> {
    match value {
        None => Ok(None),
        Some(Value::Bool(flag)) => Ok(Some(*flag)),
        Some(_) => Err(format!("{label} must be a boolean")),
    }
}

fn optional_positive(value: Option<&Value>, label: &str) -> Result<Option<f64>, String> {
    match value {
        None => Ok(None),
        Some(value) => match value.as_f64() {
            Some(number) if number.is_finite() && number > 0.0 => Ok(Some(number)),
            _ => Err(format!("{label} must be a positive number")),
        },
    }
}

fn non_empty_string(value: &Value, label: &str) -> Result<String, String> {
    match value.as_str() {
        Some(text) if !text.is_empty() => Ok(text.to_owned()),
        _ => Err(format!("{label} must be a non-empty string")),
    }
}

fn parse_string_array(value: &Value, label: &str) -> Result<Vec<String>, String> {
    let list = value
        .as_array()
        .ok_or_else(|| format!("{label} must be an array"))?;
    list.iter()
        .map(|entry| {
            entry
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{label} must contain strings"))
        })
        .collect()
}
