//! `loadHooks`：TS `hooksService.ts:214-268` + `workspaceHookSettingsModel.ts` 的装配。
//!
//! 顺序固定为：项目 snapshot → 项目 legacy agents → 项目 legacy claude → 用户 ESCode →
//! 用户 legacy agents → 用户 legacy claude。项目 snapshot 的摘要与 trust 过滤逐字对齐 TS。

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::config::{self, HookSource, HookType, RuntimeRoot, Shell};
use super::digest::{self, CanonicalEntry};
use super::legacy::legacy_hooks;
use super::{paths, trust};

pub struct HooksService {
    home: PathBuf,
}

impl HooksService {
    pub fn new(home: PathBuf) -> Self {
        Self { home }
    }

    pub async fn load(&self, params: &Value) -> Result<Value, (String, String)> {
        let workspace_text = params
            .get("workspacePath")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("loadHooks requires workspacePath"))?;
        let workspace = paths::normalize_absolute(workspace_text).map_err(invalid)?;
        let identity = params
            .get("workspaceIdentity")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map_or_else(|| paths::path_text(&workspace), str::to_owned);
        let (projects, user, persistent) = tokio::join!(
            config::read_project_sources(&workspace),
            config::read_user_source(&self.home),
            trust::read(&self.home, &identity),
        );
        let runtime_root = RuntimeRoot::resolve(
            user.iter()
                .map(|source| &source.hooks)
                .chain(projects.iter().map(|source| &source.hooks)),
        );
        let project_entries = digest::resolve_entries(&workspace, &projects, runtime_root);
        let snapshot = digest::build_snapshot(&identity, &workspace, &projects, runtime_root);
        let bundle_digest = snapshot
            .as_ref()
            .and_then(|value| value.get("bundleDigest"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let mut hooks = Vec::new();
        for entry in &project_entries {
            if let Some(source) =
                projects.get(usize::try_from(entry.source_file_index).unwrap_or(usize::MAX))
            {
                hooks.push(project_hook(
                    entry,
                    source,
                    &workspace,
                    &identity,
                    &bundle_digest,
                    &persistent.digests,
                ));
            }
        }
        hooks.extend(
            legacy_hooks(
                "agents",
                "project",
                Some(&workspace),
                &paths::join(&workspace, &[".agents", "settings.json"]),
            )
            .await,
        );
        hooks.extend(
            legacy_hooks(
                "claude",
                "project",
                Some(&workspace),
                &paths::join(&workspace, &[".claude", "settings.json"]),
            )
            .await,
        );
        if let Some(user) = &user {
            let entries =
                digest::resolve_entries(&workspace, std::slice::from_ref(user), runtime_root);
            for (index, entry) in entries.iter().enumerate() {
                hooks.push(user_hook(index, entry, &user.canonical_path, &self.home));
            }
        }
        hooks.extend(
            legacy_hooks(
                "agents",
                "user",
                None,
                &paths::join(&self.home, &[".agents", "settings.json"]),
            )
            .await,
        );
        hooks.extend(
            legacy_hooks(
                "claude",
                "user",
                None,
                &paths::join(&self.home, &[".claude", "settings.json"]),
            )
            .await,
        );
        let hooks_enabled = hooks
            .iter()
            .any(|hook| hook.get("enabled") == Some(&Value::Bool(true)));
        let mut result = Map::new();
        result.insert("hooks".to_owned(), Value::Array(hooks));
        result.insert("hooksEnabled".to_owned(), Value::Bool(hooks_enabled));
        if let Some(snapshot) = snapshot {
            result.insert("workspaceHookSnapshot".to_owned(), snapshot);
        }
        if persistent.corrupt {
            result.insert("trustStoreCorrupt".to_owned(), Value::Bool(true));
        }
        Ok(Value::Object(result))
    }

    pub async fn save(&self, params: &Value) -> Result<(), (String, String)> {
        super::write::save(&self.home, params)
            .await
            .map_err(|message| ("Error".to_owned(), message))
    }
}

fn invalid(message: impl Into<String>) -> (String, String) {
    ("InvalidRequest".to_owned(), message.into())
}

fn configured_state(entry: &CanonicalEntry, source_path: &str) -> Value {
    json!({
        "sourceRootEnabled": entry.source_root_enabled,
        "declarationEnabled": entry.declaration_enabled,
        "runtimeHooksEnabled": entry.runtime_hooks_enabled,
        "configuredEnabled": entry.configured_enabled,
        "sourcePath": source_path,
    })
}

fn base_hook(entry: &CanonicalEntry) -> Map<String, Value> {
    let mut hook = Map::new();
    hook.insert("id".to_owned(), Value::String(entry.review_item_id.clone()));
    hook.insert(
        "event".to_owned(),
        Value::String(entry.event.as_str().to_owned()),
    );
    if let Some(matcher) = &entry.matcher {
        hook.insert("matcher".to_owned(), Value::String(matcher.clone()));
    }
    hook.insert(
        "type".to_owned(),
        Value::String(entry.kind.as_str().to_owned()),
    );
    hook.insert("command".to_owned(), Value::String(entry.command.clone()));
    if entry.kind == HookType::Process {
        hook.insert(
            "args".to_owned(),
            Value::Array(
                entry
                    .args
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            ),
        );
    }
    if let Some(async_) = entry.async_ {
        hook.insert("async".to_owned(), Value::Bool(async_));
    }
    if let Some(shell) = &entry.shell {
        hook.insert(
            "shell".to_owned(),
            match shell {
                Shell::Enabled => Value::Bool(true),
                Shell::Command(command) => Value::String(command.clone()),
            },
        );
    }
    if let Some(status) = &entry.status_message {
        hook.insert("statusMessage".to_owned(), Value::String(status.clone()));
    }
    if let Some(timeout) = entry.timeout {
        hook.insert("timeout".to_owned(), digest::number(timeout));
    }
    hook.insert("enabled".to_owned(), Value::Bool(entry.configured_enabled));
    hook.insert("editable".to_owned(), Value::Bool(entry.editable));
    if !entry.custom.is_empty() {
        hook.insert("custom".to_owned(), Value::Object(entry.custom.clone()));
    }
    hook
}

fn project_hook(
    entry: &CanonicalEntry,
    source: &HookSource,
    workspace: &Path,
    identity: &str,
    bundle_digest: &str,
    trusted: &std::collections::HashSet<String>,
) -> Value {
    let mut hook = base_hook(entry);
    let source_path = paths::path_text(&source.canonical_path);
    hook.insert(
        "configuredState".to_owned(),
        configured_state(entry, &source_path),
    );
    let mut workspace_hook = match configured_state(entry, &source_path) {
        Value::Object(object) => object,
        _ => Map::new(),
    };
    workspace_hook.insert(
        "reviewItemId".to_owned(),
        Value::String(entry.review_item_id.clone()),
    );
    workspace_hook.insert(
        "sourceFileIndex".to_owned(),
        Value::from(entry.source_file_index),
    );
    workspace_hook.insert(
        "workspaceIdentity".to_owned(),
        Value::String(identity.to_owned()),
    );
    workspace_hook.insert(
        "bundleDigest".to_owned(),
        Value::String(bundle_digest.to_owned()),
    );
    workspace_hook.insert(
        "hookDeclarationDigest".to_owned(),
        Value::String(entry.hook_declaration_digest.clone()),
    );
    workspace_hook.insert(
        "trustState".to_owned(),
        Value::String(
            if trusted.contains(&entry.hook_declaration_digest) {
                "trusted_persistent"
            } else {
                "pending_trust"
            }
            .to_owned(),
        ),
    );
    hook.insert("workspaceHook".to_owned(), Value::Object(workspace_hook));
    hook.insert(
        "location".to_owned(),
        json!({
            "source": "escode",
            "scope": "project",
            "directoryPath": paths::path_text(&paths::parent(&source.canonical_path)),
            "projectPath": paths::path_text(workspace),
        }),
    );
    Value::Object(hook)
}

fn user_hook(index: usize, entry: &CanonicalEntry, source_path: &Path, home: &Path) -> Value {
    let mut hook = base_hook(entry);
    hook.insert(
        "id".to_owned(),
        Value::String(format!("hook-escode-user-{index}")),
    );
    hook.insert(
        "configuredState".to_owned(),
        configured_state(entry, &paths::path_text(source_path)),
    );
    hook.insert(
        "location".to_owned(),
        json!({
            "source": "escode",
            "scope": "user",
            "directoryPath": paths::path_text(&paths::join(home, &[".escode", "cli"])),
        }),
    );
    Value::Object(hook)
}
