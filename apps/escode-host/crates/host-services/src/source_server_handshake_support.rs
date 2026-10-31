//! 握手记录的纯投影与凭据重写（S4-j）。
//!
//! 从 `source_server_handshake_record.rs` 拆出以守住单文件 400 行门禁；不持有状态、不落盘。

use anyhow::{Context, Result};
use serde_json::Value;

use crate::credential::CredentialService;
use crate::source_server::{
    ConnectionProfile, DeclaredWorkspace, SourceServerRecord, WorkspaceHistoryEntry,
};

use super::CredentialRollback;

pub(super) async fn rekey_credentials(
    current: &ConnectionProfile,
    new_profile_id: &str,
    credentials: Option<&CredentialService>,
    rollback: &mut CredentialRollback,
) -> Result<Value> {
    let mut target = current.target.clone();
    if current.transport != "ssh" {
        return Ok(target);
    }
    let Some(service) = credentials else {
        return Ok(target);
    };
    let object = target
        .as_object_mut()
        .context("SSH Connection Profile target must be an object")?;
    for (field, kind) in [
        ("passwordCredentialKey", "password"),
        (
            "privateKeyPassphraseCredentialKey",
            "private-key-passphrase",
        ),
    ] {
        let old_key = object
            .get(field)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(std::string::ToString::to_string);
        let Some(old_key) = old_key else {
            continue;
        };
        let secret = service
            .load(&old_key)
            .await
            .with_context(|| format!("load credential {old_key}"))?;
        let Some(secret) = secret else {
            object.remove(field);
            continue;
        };
        let new_key = format!("connection-profile:{new_profile_id}:{kind}");
        rollback.save(service, &new_key, &secret).await?;
        object.insert(field.to_owned(), Value::String(new_key));
    }
    Ok(target)
}

pub(super) fn upsert_server(servers: &mut Vec<SourceServerRecord>, server: SourceServerRecord) {
    match servers
        .iter()
        .position(|candidate| candidate.source_server_id == server.source_server_id)
    {
        Some(index) => servers[index] = server,
        None => servers.push(server),
    }
}

pub(super) fn rewrite_history(
    history: &mut [WorkspaceHistoryEntry],
    profile_id: &str,
    source_server_id: &str,
) {
    for entry in history {
        if entry.profile_id == profile_id {
            entry.source_server_id = Some(source_server_id.to_owned());
            entry.workspace_identity = Some(server_workspace_identity(
                source_server_id,
                &entry.workspace_path,
            ));
        }
    }
}

pub(super) fn default_display_name(
    profile: &ConnectionProfile,
    record: &super::HandshakeRecord,
) -> String {
    if profile.transport == "ssh"
        && let Some(alias) = profile
            .target
            .get("sshConfigAlias")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    {
        return alias.to_owned();
    }
    let parts: Vec<&str> = [record.hostname.as_deref(), record.os_user.as_deref()]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect();
    if parts.is_empty() {
        record.source_server_id.clone()
    } else {
        parts.join(" · ")
    }
}

pub(super) fn project_declared_workspaces(
    source_server_id: &str,
    workspaces: &[super::DeclaredWorkspaceInput],
) -> Vec<DeclaredWorkspace> {
    let mut result = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for workspace in workspaces {
        let path = workspace.path.trim();
        if path.is_empty() {
            continue;
        }
        let identity = server_workspace_identity(source_server_id, path);
        if workspace
            .workspace_identity
            .as_deref()
            .is_some_and(|remote| remote != identity)
        {
            escode_host_control::logging::debug(
                "source-server",
                format!(
                    "declared workspace identity overridden by canonical: server={source_server_id} path={path}"
                ),
            );
        }
        if !seen.insert(identity.clone()) {
            continue;
        }
        result.push(DeclaredWorkspace {
            workspace_path: path.to_owned(),
            workspace_identity: identity,
            label: workspace
                .label
                .as_deref()
                .map(str::trim)
                .filter(|label| !label.is_empty())
                .map(str::to_owned)
                .or_else(|| Some(workspace_label(path))),
        });
        if result.len() >= 20 {
            break;
        }
    }
    result
}

pub(super) fn server_workspace_identity(source_server_id: &str, workspace_path: &str) -> String {
    let server_id: String = source_server_id
        .trim()
        .to_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '-'
            }
        })
        .collect();
    let server_id = server_id.trim_matches('-');
    let normalized: String = workspace_path
        .replace('\\', "/")
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    format!("remote:server:{server_id}:/{normalized}")
}

pub(super) fn workspace_label(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    let label = trimmed.rsplit('/').next().unwrap_or_default();
    if label.is_empty() {
        path.to_owned()
    } else {
        label.to_owned()
    }
}
