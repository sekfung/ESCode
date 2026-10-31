//! 握手身份记录与 Profile 重绑定（S4-j，docs/specs/source-server-remote-rust-host.md §11.12）。
//!
//! 与 Node `SourceServerDirectory.recordHandshake` 对等：身份不变时原位绑定 Source 并重写该
//! Profile 的 workspace history；身份变化只允许在用户确认后提交，未确认时严格零写入。

use std::fmt;

use anyhow::{Context, Result, bail};
use serde_json::json;

use crate::credential::CredentialService;

use super::super::{ConnectionProfile, SourceServerRecord, SourceServerRegistry};
use super::handshake_support::{
    default_display_name, project_declared_workspaces, rekey_credentials, rewrite_history,
    upsert_server,
};
use super::{CredentialRollback, credential_keys};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclaredWorkspaceInput {
    pub path: String,
    pub workspace_identity: Option<String>,
    pub label: Option<String>,
}

#[derive(Clone, Debug)]
pub struct HandshakeRecord {
    pub source_server_id: String,
    pub os_user: Option<String>,
    pub hostname: Option<String>,
    pub version: String,
    pub connected_at: u64,
    pub workspaces: Vec<DeclaredWorkspaceInput>,
}

#[derive(Clone, Debug)]
pub struct HandshakeOutcome {
    pub identity_changed: bool,
    pub effective_profile: ConnectionProfile,
    /// 控制器行身份迁移尚未落地；保持空数组，不伪造失败。
    pub migration_failures: Vec<String>,
}

/// 身份变化待确认；RPC 层据此透传 `source-server-identity-change-pending`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentityChangePending {
    pub profile_id: String,
    pub pending_source_server_id: String,
}

impl fmt::Display for IdentityChangePending {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .write_str("Source Server identity changed after reinstall and requires confirmation")
    }
}

impl std::error::Error for IdentityChangePending {}

impl SourceServerRegistry {
    /// 记录一次成功握手。`confirm` 只影响身份变化分支；身份不变时始终原位更新。
    pub async fn record_handshake(
        &self,
        profile_id: &str,
        record: &HandshakeRecord,
        confirm: bool,
        credentials: Option<&CredentialService>,
    ) -> Result<HandshakeOutcome> {
        let _guard = self.write_lock.lock().await;
        let current = self
            .profile(profile_id)
            .with_context(|| format!("Unknown Connection Profile: {profile_id}"))?;
        if current.lifecycle_state == "retired" {
            bail!("Connection Profile is retired: {profile_id}");
        }
        let identity_changed = current
            .source_server_id
            .as_deref()
            .is_some_and(|previous| previous != record.source_server_id);
        if identity_changed && !confirm {
            return Ok(HandshakeOutcome {
                identity_changed: true,
                effective_profile: current,
                migration_failures: Vec::new(),
            });
        }
        if identity_changed {
            return self
                .commit_identity_replacement(&current, record, credentials)
                .await;
        }
        self.bind_existing_identity(&current, record).await
    }

    async fn bind_existing_identity(
        &self,
        current: &ConnectionProfile,
        record: &HandshakeRecord,
    ) -> Result<HandshakeOutcome> {
        let (mut profiles, mut servers, mut history) = {
            let state = self.lock();
            (
                state.profiles.clone(),
                state.servers.clone(),
                state.workspace_history.clone(),
            )
        };
        let index = profiles
            .iter()
            .position(|profile| profile.profile_id == current.profile_id)
            .with_context(|| format!("Unknown Connection Profile: {}", current.profile_id))?;
        let mut profile = current.clone();
        profile.source_server_id = Some(record.source_server_id.clone());
        profile.last_connected_at = Some(record.connected_at);
        profile.last_connect_error = None;
        profile.lifecycle_state = "active".to_owned();
        profiles[index] = profile.clone();

        let declared = project_declared_workspaces(&record.source_server_id, &record.workspaces);
        let previous_server = servers
            .iter()
            .find(|server| server.source_server_id == record.source_server_id)
            .cloned();
        let server = SourceServerRecord {
            source_server_id: record.source_server_id.clone(),
            display_name: previous_server.as_ref().map_or_else(
                || default_display_name(&profile, record),
                |server| server.display_name.clone(),
            ),
            os_user: record.os_user.clone(),
            hostname: record.hostname.clone(),
            last_seen_version: Some(record.version.clone()),
            // 握手只确认身份可达，尚未通过 Provider Provisioning 屏障；在线时间由
            // set_connection_state(online) 在屏障通过后推进，失败首连不得留下 lastOnlineAt。
            last_online_at: previous_server
                .as_ref()
                .and_then(|server| server.last_online_at),
            preferred_profile_id: previous_server
                .as_ref()
                .and_then(|server| server.preferred_profile_id.clone())
                .or_else(|| Some(profile.profile_id.clone())),
            install_state: "installed".to_owned(),
            declared_workspaces: declared,
        };
        upsert_server(&mut servers, server);
        rewrite_history(&mut history, &profile.profile_id, &record.source_server_id);
        self.persist(json!({
            "connectionProfiles": &profiles,
            "sourceServers": &servers,
            "sourceServerWorkspaceHistory": &history,
        }))
        .await?;
        let profiles_for_state = profiles.clone();
        let servers_for_state = servers.clone();
        let history_for_state = history.clone();
        self.apply_state(|state| {
            state.profiles = profiles_for_state;
            state.servers = servers_for_state;
            state.workspace_history = history_for_state;
        });
        Ok(HandshakeOutcome {
            identity_changed: false,
            effective_profile: profile,
            migration_failures: Vec::new(),
        })
    }

    async fn commit_identity_replacement(
        &self,
        current: &ConnectionProfile,
        record: &HandshakeRecord,
        credentials: Option<&CredentialService>,
    ) -> Result<HandshakeOutcome> {
        let previous_source_server_id = current
            .source_server_id
            .clone()
            .context("identity replacement requires a bound Connection Profile")?;
        let new_profile_id = uuid::Uuid::new_v4().to_string();
        if new_profile_id == current.profile_id {
            bail!("Identity replacement requires a new Connection Profile id");
        }
        let keys = credential_keys(current);
        if !keys.is_empty() && credentials.is_none() {
            bail!(
                "source server credentials need dataBaseDir: Main must pass dataBaseDir in rpcListen"
            );
        }
        let mut rollback = CredentialRollback::default();
        let target =
            match rekey_credentials(current, &new_profile_id, credentials, &mut rollback).await {
                Ok(target) => target,
                Err(error) => {
                    if let Some(service) = credentials {
                        rollback.restore(service).await;
                    }
                    return Err(error);
                }
            };
        let mut retired = current.clone();
        retired.lifecycle_state = "retired".to_owned();
        retired.retired_reason = Some("source-reinstalled".to_owned());
        retired.retired_at = Some(record.connected_at);
        let mut profile = current.clone();
        profile.profile_id = new_profile_id.clone();
        profile.source_server_id = Some(record.source_server_id.clone());
        profile.target = target;
        profile.lifecycle_state = "active".to_owned();
        profile.retired_reason = None;
        profile.retired_at = None;
        profile.last_connected_at = Some(record.connected_at);
        profile.last_connect_error = None;
        profile.created_at = record.connected_at;

        let (mut profiles, mut servers, history) = {
            let state = self.lock();
            (
                state.profiles.clone(),
                state.servers.clone(),
                state.workspace_history.clone(),
            )
        };
        let index = profiles
            .iter()
            .position(|candidate| candidate.profile_id == current.profile_id)
            .with_context(|| format!("Unknown Connection Profile: {}", current.profile_id))?;
        profiles[index] = retired;
        profiles.push(profile.clone());
        if let Some(server) = servers
            .iter_mut()
            .find(|server| server.source_server_id == previous_source_server_id)
        {
            server.install_state = "uninstalled".to_owned();
            server.preferred_profile_id = profiles
                .iter()
                .find(|candidate| {
                    candidate.lifecycle_state != "retired"
                        && candidate.source_server_id.as_deref()
                            == Some(previous_source_server_id.as_str())
                })
                .map(|candidate| candidate.profile_id.clone());
        }
        let existing = servers
            .iter()
            .find(|server| server.source_server_id == record.source_server_id)
            .cloned();
        let server = SourceServerRecord {
            source_server_id: record.source_server_id.clone(),
            display_name: existing.as_ref().map_or_else(
                || default_display_name(&profile, record),
                |server| server.display_name.clone(),
            ),
            os_user: record.os_user.clone(),
            hostname: record.hostname.clone(),
            last_seen_version: Some(record.version.clone()),
            // 同上：保留新身份此前的在线时间；从未在线的 Source 必须保持 None，
            // 否则身份替换后的首连屏障会被握手提前写入的时间戳绕过。
            last_online_at: existing.as_ref().and_then(|server| server.last_online_at),
            preferred_profile_id: Some(new_profile_id),
            install_state: "installed".to_owned(),
            declared_workspaces: project_declared_workspaces(
                &record.source_server_id,
                &record.workspaces,
            ),
        };
        upsert_server(&mut servers, server);
        let patch = json!({
            "connectionProfiles": &profiles,
            "sourceServers": &servers,
            "sourceServerWorkspaceHistory": &history,
        });
        if let Err(error) = self.persist(patch).await {
            if let Some(service) = credentials {
                rollback.restore(service).await;
            }
            return Err(error);
        }
        let profiles_for_state = profiles.clone();
        let servers_for_state = servers.clone();
        let history_for_state = history.clone();
        self.apply_state(|state| {
            state.profiles = profiles_for_state;
            state.servers = servers_for_state;
            state.workspace_history = history_for_state;
        });
        Ok(HandshakeOutcome {
            identity_changed: true,
            effective_profile: profile,
            migration_failures: Vec::new(),
        })
    }
}

#[cfg(test)]
#[path = "source_server_handshake_record_tests.rs"]
mod tests;
