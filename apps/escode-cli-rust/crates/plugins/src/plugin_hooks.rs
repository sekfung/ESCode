//! 插件 hook 源发现、明细与 components 事件名（docs/specs/rust-plugins.md 第 4 期 4b）。
//! TS 基准：`adapters/src/plugins/hook-sources.ts::listPluginHookSources` +
//! `index.ts::inspectPluginHooks / parsePluginHookEvents / toPluginHookDetail`。
//!
//! 发现阶段每个插件只读一次，`hook_details` 供 `plugins/list|overview` 复用，`events` 供 components 的
//! hook 分组；两条投影不再各自扫盘。

use super::extension_plugins::diagnostic;
use escode_cli_domain::json_order::Json;
use serde_json::{Number, Value, json};
use std::path::{Path, PathBuf};

/// TS `HookEventName` 全集（`contracts/src/hooks/index.ts`）。
const HOOK_EVENTS: [&str; 7] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "PostToolUseFailure",
    "Stop",
];

pub struct Inspection {
    pub details: Vec<Value>,
    pub events: Vec<String>,
}

pub(crate) struct Source {
    pub(crate) raw: Json,
    pub(crate) source_path: String,
    pub(crate) wrapper: bool,
}

/// TS `inspectPluginHooks`：来源发现 + 解析 + 校验，诊断按发现顺序追加到 `diagnostics`。
pub async fn inspect(
    id: &str,
    root: &Path,
    manifest_path: &Path,
    manifest_hooks: Option<&Json>,
    diagnostics: &mut Vec<Value>,
) -> Inspection {
    let mut sources: Vec<Source> = vec![];
    let mut loaded: Vec<PathBuf> = vec![];

    let standard = root.join("hooks").join("hooks.json");
    if tokio::fs::metadata(&standard)
        .await
        .is_ok_and(|meta| meta.is_file())
        && let Some(raw) = load(&standard, id, diagnostics).await
    {
        sources.push(Source {
            raw,
            source_path: standard.to_string_lossy().into_owned(),
            wrapper: true,
        });
        loaded.push(realpath(&standard).await);
    }

    if let Some(hooks) = manifest_hooks {
        let specs: Vec<&Json> = match hooks {
            Json::Array(items) => items.iter().collect(),
            other => vec![other],
        };
        for spec in specs {
            let Some(spec_path) = spec.as_str() else {
                sources.push(Source {
                    raw: spec.clone(),
                    source_path: manifest_path.to_string_lossy().into_owned(),
                    wrapper: false,
                });
                continue;
            };
            let Some(path) = super::plugin_install::resolve_inside(root, spec_path) else {
                diagnostics.push(diagnostic(
                    "plugin_component_path_invalid",
                    format!("Plugin hooks path escapes plugin root: {spec_path}"),
                    Some(id),
                    "error",
                ));
                continue;
            };
            if !tokio::fs::metadata(&path)
                .await
                .is_ok_and(|meta| meta.is_file())
            {
                diagnostics.push(diagnostic(
                    "plugin_hook_read_failed",
                    format!("Plugin hooks file not found: {spec_path}"),
                    Some(id),
                    "error",
                ));
                continue;
            }
            let real = realpath(&path).await;
            if loaded.contains(&real) {
                diagnostics.push(diagnostic(
                    "plugin_hook_invalid",
                    format!("Duplicate plugin hooks file ignored: {spec_path}"),
                    Some(id),
                    "warning",
                ));
                continue;
            }
            if let Some(raw) = load(&path, id, diagnostics).await {
                sources.push(Source {
                    raw,
                    source_path: path.to_string_lossy().into_owned(),
                    wrapper: true,
                });
                loaded.push(real);
            }
        }
    }

    let mut inspection = Inspection {
        details: vec![],
        events: vec![],
    };
    for source in sources {
        parse_source(id, source, &mut inspection, diagnostics);
    }
    inspection
}

pub(crate) fn parse_source(
    id: &str,
    source: Source,
    out: &mut Inspection,
    diagnostics: &mut Vec<Value>,
) {
    let hooks_root = if source.wrapper {
        source.raw.get("hooks").filter(|value| value.is_object())
    } else {
        Some(&source.raw).filter(|value| value.is_object())
    };
    let Some(Json::Object(events)) = hooks_root else {
        diagnostics.push(diagnostic(
            "plugin_hook_invalid",
            if source.wrapper {
                "Plugin hooks file must contain a hooks object"
            } else {
                "Plugin manifest hooks entry must be an object, a path, or an array"
            }
            .to_owned(),
            Some(id),
            "error",
        ));
        return;
    };
    for (event, matchers) in events {
        if !HOOK_EVENTS.contains(&event.as_str()) {
            diagnostics.push(diagnostic(
                "plugin_hook_unsupported_event",
                format!("Plugin hook event is not supported by this ESCode runtime: {event}"),
                Some(id),
                "warning",
            ));
            continue;
        }
        if !out.events.contains(event) {
            out.events.push(event.clone());
        }
        let Some(matchers) = matchers.as_array() else {
            diagnostics.push(diagnostic(
                "plugin_hook_invalid",
                format!("Plugin hook event must be an array: {event}"),
                Some(id),
                "error",
            ));
            continue;
        };
        for matcher in matchers {
            match validate_matcher(matcher) {
                Ok((label, hooks)) => {
                    for hook in hooks {
                        out.details.push(detail(
                            event,
                            label.as_deref(),
                            &hook,
                            &source.source_path,
                        ));
                    }
                }
                Err(reason) => diagnostics.push(diagnostic(
                    "plugin_hook_invalid",
                    format!("Invalid plugin hook matcher for {event}: {reason}"),
                    Some(id),
                    "error",
                )),
            }
        }
    }
}

struct Hook {
    kind: &'static str,
    command: String,
    args: Option<Vec<String>>,
    asynchronous: Option<bool>,
    shell: Option<Value>,
    timeout: Option<Number>,
    timeout_ms: Option<Number>,
    status_message: Option<String>,
}

/// TS `HookMatcherConfigSchema`：`hooks` 非空，`matcher` 可缺省或字符串；未知键忽略。
fn validate_matcher(raw: &Json) -> Result<(Option<String>, Vec<Hook>), String> {
    if !raw.is_object() {
        return Err("expected an object".to_owned());
    }
    let label = match raw.get("matcher") {
        None => None,
        Some(Json::String(value)) => Some(value.clone()),
        Some(_) => return Err("matcher: expected string".to_owned()),
    };
    let Some(hooks) = raw.get("hooks") else {
        return Err("hooks: required".to_owned());
    };
    let Some(items) = hooks.as_array() else {
        return Err("hooks: expected array".to_owned());
    };
    if items.is_empty() {
        return Err("hooks: array must contain at least 1 item".to_owned());
    }
    let mut parsed = Vec::with_capacity(items.len());
    for item in items {
        parsed.push(validate_hook(item)?);
    }
    Ok((label, parsed))
}

/// TS `HookConfigSchema` 判别联合（process / command）；共享字段为 `command`、`timeoutMs`、`statusMessage`。
fn validate_hook(raw: &Json) -> Result<Hook, String> {
    if !raw.is_object() {
        return Err("hook: expected an object".to_owned());
    }
    let command = match raw.get("command") {
        Some(Json::String(value)) if !value.is_empty() => value.clone(),
        _ => return Err("command: expected a non-empty string".to_owned()),
    };
    let status_message = optional_string(raw, "statusMessage")?;
    let timeout_ms = optional_positive_number(raw, "timeoutMs", true)?;
    // `enabled` 是两种 hook 共有的已知键（不进 hookDetails），非法类型同样要让整条 matcher 失效。
    optional_bool(raw, "enabled")?;
    match raw.get("type").and_then(Json::as_str) {
        Some("process") => Ok(Hook {
            kind: "process",
            command,
            args: optional_string_array(raw, "args")?,
            asynchronous: None,
            shell: None,
            timeout: None,
            timeout_ms,
            status_message,
        }),
        Some("command") => Ok(Hook {
            kind: "command",
            command,
            args: None,
            asynchronous: optional_bool(raw, "async")?,
            shell: optional_shell(raw)?,
            timeout: optional_positive_number(raw, "timeout", false)?,
            timeout_ms,
            status_message,
        }),
        _ => Err("type: expected \"command\" or \"process\"".to_owned()),
    }
}

fn optional_bool(raw: &Json, key: &str) -> Result<Option<bool>, String> {
    match raw.get(key) {
        None => Ok(None),
        Some(Json::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(format!("{key}: expected boolean")),
    }
}

fn optional_string(raw: &Json, key: &str) -> Result<Option<String>, String> {
    match raw.get(key) {
        None => Ok(None),
        Some(Json::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(format!("{key}: expected string")),
    }
}

fn optional_string_array(raw: &Json, key: &str) -> Result<Option<Vec<String>>, String> {
    match raw.get(key) {
        None => Ok(None),
        Some(Json::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("{key}: expected array of strings"))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(_) => Err(format!("{key}: expected array of strings")),
    }
}

/// `shell: true | 非空字符串`（`false` / 空串 / null 均非法）。
fn optional_shell(raw: &Json) -> Result<Option<Value>, String> {
    match raw.get("shell") {
        None => Ok(None),
        Some(Json::Bool(true)) => Ok(Some(Value::Bool(true))),
        Some(Json::String(value)) if !value.is_empty() => Ok(Some(Value::String(value.clone()))),
        Some(_) => Err("shell: expected true or a non-empty string".to_owned()),
    }
}

/// `timeoutMs: 正整数` / `timeout: > 0`。保留原始 JSON 数字表示。
fn optional_positive_number(
    raw: &Json,
    key: &str,
    integer: bool,
) -> Result<Option<Number>, String> {
    match raw.get(key) {
        None => Ok(None),
        Some(Json::Number(number)) => {
            let value = number
                .as_f64()
                .ok_or_else(|| format!("{key}: expected number"))?;
            if !(value.is_finite() && value > 0.0) {
                return Err(format!("{key}: expected a positive number"));
            }
            if integer && value.fract() != 0.0 {
                return Err(format!("{key}: expected an integer"));
            }
            Ok(Some(number.clone()))
        }
        Some(_) => Err(format!("{key}: expected number")),
    }
}

/// TS `toPluginHookDetail`：process 只带 `args`，command 只带 `async` / `shell` / `timeout`。
fn detail(event: &str, matcher: Option<&str>, hook: &Hook, source_path: &str) -> Value {
    let mut value = json!({
        "command": hook.command.clone(),
        "event": event,
        "runnable": true,
        "sourcePath": source_path,
        "type": hook.kind,
    });
    if let Some(matcher) = matcher {
        value["matcher"] = matcher.into();
    }
    if let Some(status_message) = &hook.status_message {
        value["statusMessage"] = status_message.clone().into();
    }
    if let Some(timeout_ms) = &hook.timeout_ms {
        value["timeoutMs"] = Value::Number(timeout_ms.clone());
    }
    if hook.kind == "process" {
        if let Some(args) = &hook.args {
            value["args"] = Value::Array(args.iter().cloned().map(Value::String).collect());
        }
        return value;
    }
    if let Some(asynchronous) = hook.asynchronous {
        value["async"] = asynchronous.into();
    }
    if let Some(shell) = &hook.shell {
        value["shell"] = shell.clone();
    }
    if let Some(timeout) = &hook.timeout {
        value["timeout"] = Value::Number(timeout.clone());
    }
    value
}

/// TS `loadPluginHookSource`：读取/解析失败只记诊断并跳过该来源。
async fn load(path: &Path, id: &str, diagnostics: &mut Vec<Value>) -> Option<Json> {
    let text = match tokio::fs::read_to_string(path).await {
        Ok(text) => text,
        Err(error) => {
            diagnostics.push(diagnostic(
                "plugin_hook_read_failed",
                error.to_string(),
                Some(id),
                "error",
            ));
            return None;
        }
    };
    match Json::parse(&text) {
        Some(raw) => Some(raw),
        None => {
            // 修复：文案来自 serde 而非 V8（与 manifest 解析同类差异），code/severity 对齐即可。
            let error = serde_json::from_str::<serde_json::Value>(&text)
                .err()
                .map_or_else(|| "invalid JSON".to_owned(), |error| error.to_string());
            diagnostics.push(diagnostic(
                "plugin_hook_read_failed",
                format!("Unexpected token in JSON: {error}"),
                Some(id),
                "error",
            ));
            None
        }
    }
}

async fn realpath(path: &Path) -> PathBuf {
    tokio::fs::canonicalize(path)
        .await
        .unwrap_or_else(|_| path.to_owned())
}
