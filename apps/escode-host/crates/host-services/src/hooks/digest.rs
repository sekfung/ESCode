//! Workspace hook 声明的规范化与摘要（TS `workspace-hook-digest.ts` 逐字对齐）。
//!
//! 摘要输入是 `JSON.stringify(payload)` 的规范数组：字段顺序、`null` matcher、
//! `["unset"]`/`["set", value]` 编码、整数取整都必须一致，否则既有 trust 记录全部失效。

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::config::{HookDefinition, HookEvent, HookSource, HookType, RuntimeRoot, Shell};
use super::paths;

#[derive(Clone, Debug)]
pub struct CanonicalEntry {
    pub review_item_id: String,
    pub event: HookEvent,
    pub matcher_index: u64,
    pub hook_index: u64,
    pub source_file_index: u64,
    pub source_relative_path: String,
    pub matcher: Option<String>,
    pub kind: HookType,
    pub command: String,
    pub args: Option<Vec<String>>,
    pub async_: Option<bool>,
    pub shell: Option<Shell>,
    pub resolved_timeout_ms: u64,
    pub resolved_max_output_bytes: u64,
    pub status_message: Option<String>,
    pub source_root_enabled: bool,
    pub declaration_enabled: bool,
    pub runtime_hooks_enabled: bool,
    pub configured_enabled: bool,
    pub editable: bool,
    pub hook_declaration_digest: String,
    /// `toHook` 的 `timeout`（秒）；不参与 snapshot/digest。
    pub timeout: Option<f64>,
    /// `getCustomHookFields(raw)`；不参与 snapshot/digest。
    pub custom: serde_json::Map<String, Value>,
}

pub fn resolve_entries(
    workspace_path: &std::path::Path,
    sources: &[HookSource],
    runtime_root: RuntimeRoot,
) -> Vec<CanonicalEntry> {
    let mut entries = Vec::new();
    for (source_file_index, source) in sources.iter().enumerate() {
        let source_relative_path =
            paths::relative_source_path(workspace_path, &source.canonical_path);
        for event in HookEvent::ALL {
            let Some(matchers) = source.hooks.events[super::config::event_index(event)].as_deref()
            else {
                continue;
            };
            for (matcher_index, matcher) in matchers.iter().enumerate() {
                for (hook_index, hook) in matcher.hooks.iter().enumerate() {
                    let (source_root_enabled, declaration_enabled, configured_enabled) =
                        super::config::configured_gates(
                            source.hooks.enabled,
                            hook.enabled,
                            runtime_root.enabled,
                        );
                    let resolved_timeout_ms =
                        super::config::resolved_timeout_ms(hook, runtime_root.timeout_ms);
                    let digest = declaration_digest(
                        &source_relative_path,
                        source.discovery_order,
                        event,
                        matcher.matcher.as_deref(),
                        matcher_index,
                        hook_index,
                        hook,
                        resolved_timeout_ms,
                        runtime_root.max_output_bytes,
                    );
                    entries.push(CanonicalEntry {
                        review_item_id: format!(
                            "workspace-hook-{source_file_index}-{}-{matcher_index}-{hook_index}",
                            event.as_str()
                        ),
                        event,
                        matcher_index: matcher_index as u64,
                        hook_index: hook_index as u64,
                        source_file_index: source_file_index as u64,
                        source_relative_path: source_relative_path.clone(),
                        matcher: matcher.matcher.clone(),
                        kind: hook.kind,
                        command: hook.command.clone(),
                        args: match hook.kind {
                            HookType::Process => {
                                hook.args.as_ref().filter(|args| !args.is_empty()).cloned()
                            }
                            HookType::Command => None,
                        },
                        async_: match hook.kind {
                            HookType::Command if hook.async_ == Some(true) => Some(true),
                            _ => None,
                        },
                        shell: match hook.kind {
                            HookType::Command => hook.shell.clone(),
                            HookType::Process => None,
                        },
                        resolved_timeout_ms,
                        resolved_max_output_bytes: runtime_root.max_output_bytes.round() as u64,
                        status_message: hook.status_message.clone(),
                        source_root_enabled,
                        declaration_enabled,
                        runtime_hooks_enabled: runtime_root.enabled,
                        configured_enabled,
                        editable: source.editable,
                        hook_declaration_digest: digest,
                        timeout: super::config::hook_timeout_seconds(hook),
                        custom: hook.custom.clone(),
                    });
                }
            }
        }
    }
    entries
}

pub fn build_snapshot(
    workspace_identity: &str,
    workspace_path: &std::path::Path,
    sources: &[HookSource],
    runtime_root: RuntimeRoot,
) -> Option<Value> {
    let hooks = resolve_entries(workspace_path, sources, runtime_root);
    if hooks.is_empty() {
        return None;
    }
    let source_files: Vec<Value> = sources
        .iter()
        .map(|source| {
            json!({
                "canonicalPath": paths::path_text(&source.canonical_path),
                "baseDir": paths::path_text(&source.base_dir),
                "discoveryOrder": source.discovery_order,
                "configFileKind": source.config_file_kind,
                "explicitProjectConfig": source.explicit_project_config,
                "editable": source.editable,
                "hooksRoot": hooks_root(source),
            })
        })
        .collect();
    let bundle_digest = bundle_digest(workspace_path, sources, &hooks);
    let hook_values: Vec<Value> = hooks.iter().map(entry_json).collect();
    Some(json!({
        "schemaVersion": 1,
        "workspaceIdentity": workspace_identity,
        "discoveredAt": crate::onboarding::iso_now(),
        "sourceFiles": source_files,
        "hooks": hook_values,
        "digestAlgorithm": "sha256",
        "bundleDigest": bundle_digest,
    }))
}

pub fn bundle_digest(
    workspace_path: &std::path::Path,
    sources: &[HookSource],
    hooks: &[CanonicalEntry],
) -> String {
    let source_payload: Vec<Value> = sources
        .iter()
        .map(|source| {
            json!([
                paths::relative_source_path(workspace_path, &source.canonical_path),
                source.discovery_order,
                source.config_file_kind,
                source.explicit_project_config,
                canonical_optional(source.hooks.enabled.map(Value::Bool)),
                canonical_optional(source.hooks.timeout_ms.map(number)),
                canonical_optional(source.hooks.max_output_bytes.map(number)),
            ])
        })
        .collect();
    let hook_payload: Vec<Value> = hooks
        .iter()
        .map(|hook| {
            json!([
                hook.hook_declaration_digest,
                hook.source_root_enabled,
                hook.declaration_enabled,
                hook.runtime_hooks_enabled,
                hook.configured_enabled,
            ])
        })
        .collect();
    sha256(&json!([
        "workspace-hook-bundle",
        1,
        source_payload,
        hook_payload
    ]))
}

#[allow(clippy::too_many_arguments)]
fn declaration_digest(
    source_relative_path: &str,
    source_discovery_order: u64,
    event: HookEvent,
    matcher: Option<&str>,
    matcher_index: usize,
    hook_index: usize,
    hook: &HookDefinition,
    resolved_timeout_ms: u64,
    resolved_max_output_bytes: f64,
) -> String {
    let execution = match hook.kind {
        HookType::Process => json!([
            "process",
            hook.command,
            hook.args.clone().unwrap_or_default()
        ]),
        HookType::Command => json!([
            "command",
            hook.command,
            hook.async_ == Some(true),
            match &hook.shell {
                None => json!(["unset"]),
                Some(Shell::Enabled) => json!(["true"]),
                Some(Shell::Command(command)) => json!(["string", command]),
            },
        ]),
    };
    sha256(&json!([
        "workspace-hook-declaration",
        1,
        source_relative_path,
        source_discovery_order,
        event.as_str(),
        matcher,
        matcher_index as u64,
        hook_index as u64,
        execution,
        resolved_timeout_ms,
        resolved_max_output_bytes.round() as u64,
    ]))
}

fn entry_json(entry: &CanonicalEntry) -> Value {
    let mut value = json!({
        "reviewItemId": entry.review_item_id,
        "event": entry.event.as_str(),
        "matcherIndex": entry.matcher_index,
        "hookIndex": entry.hook_index,
        "sourceFileIndex": entry.source_file_index,
        "sourceRelativePath": entry.source_relative_path,
        "type": entry.kind.as_str(),
        "command": entry.command,
        "resolvedTimeoutMs": entry.resolved_timeout_ms,
        "resolvedMaxOutputBytes": entry.resolved_max_output_bytes,
        "sourceRootEnabled": entry.source_root_enabled,
        "declarationEnabled": entry.declaration_enabled,
        "runtimeHooksEnabled": entry.runtime_hooks_enabled,
        "configuredEnabled": entry.configured_enabled,
        "editable": entry.editable,
        "declarationDigestAlgorithm": "sha256",
        "hookDeclarationDigest": entry.hook_declaration_digest,
    });
    if let Some(matcher) = &entry.matcher {
        value["matcher"] = Value::String(matcher.clone());
    } else {
        value["matcher"] = Value::Null;
    }
    if let Some(args) = &entry.args {
        value["args"] = json!(args);
    }
    if let Some(async_) = entry.async_ {
        value["async"] = Value::Bool(async_);
    }
    if let Some(shell) = &entry.shell {
        value["shell"] = match shell {
            Shell::Enabled => Value::Bool(true),
            Shell::Command(command) => Value::String(command.clone()),
        };
    }
    if let Some(status_message) = &entry.status_message {
        value["statusMessage"] = Value::String(status_message.clone());
    }
    value
}

fn hooks_root(source: &HookSource) -> Value {
    let mut root = serde_json::Map::new();
    if let Some(enabled) = source.hooks.enabled {
        root.insert("enabled".to_owned(), Value::Bool(enabled));
    }
    if let Some(timeout_ms) = source.hooks.timeout_ms {
        root.insert("timeoutMs".to_owned(), number(timeout_ms));
    }
    if let Some(max_output_bytes) = source.hooks.max_output_bytes {
        root.insert("maxOutputBytes".to_owned(), number(max_output_bytes));
    }
    Value::Object(root)
}

fn canonical_optional(value: Option<Value>) -> Value {
    match value {
        Some(value) => json!(["set", value]),
        None => json!(["unset"]),
    }
}

/// JS 数字：整数值输出为整数（`JSON.stringify(60000)` 不是 `60000.0`）。
pub fn number(value: f64) -> Value {
    if value.is_finite()
        && value.fract() == 0.0
        && value >= i64::MIN as f64
        && value <= i64::MAX as f64
    {
        Value::from(value as i64)
    } else {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

pub fn sha256(value: &Value) -> String {
    let text = serde_json::to_string(value).unwrap_or_default();
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}
