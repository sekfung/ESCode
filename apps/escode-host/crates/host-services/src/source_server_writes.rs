//! `source-server-management` 写面（docs/specs/source-server-remote-rust-host.md §9.8）。
//!
//! 唯一写者：本模块在 registry 写锁内按当前内存快照生成新的 settings 数组，经
//! [`settings::apply_patch`] 落盘成功后才更新内存、递增 `generation` 并发布
//! `onDidChangeDirectory`。凭据与 settings 同处一个回滚边界；失败不留下半截状态。
//!
//! 只写四个顶层键：`sourceServers` / `connectionProfiles` / `sourceServerWorkspaceHistory` /
//! `remoteConversationWorkspaceBySourceServerId`，其它键原样保留。

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

use crate::credential::CredentialService;
use crate::settings;

use super::{ConnectionProfile, RegistryState, SourceServerRegistry};

#[path = "source_server_conversation_workspace.rs"]
mod conversation_workspace;
#[path = "source_server_handshake_record.rs"]
mod handshake_record;
#[path = "source_server_handshake_support.rs"]
mod handshake_support;
#[path = "source_server_profile_ensure.rs"]
mod profile_ensure;
#[path = "source_server_profile_update.rs"]
mod profile_update;
#[path = "source_server_target_normalize.rs"]
mod target_normalize;

pub use handshake_record::{
    DeclaredWorkspaceInput, HandshakeOutcome, HandshakeRecord, IdentityChangePending,
};
pub use profile_update::UpdateProfileInput;

/// 凭据回滚记录：第一次触碰某个 key 时记下原值，失败时按原值恢复。
#[derive(Default)]
pub(super) struct CredentialRollback {
    previous: Vec<(String, Option<String>)>,
}

impl CredentialRollback {
    pub(super) async fn remember(&mut self, service: &CredentialService, key: &str) -> Result<()> {
        if self.previous.iter().any(|(known, _)| known == key) {
            return Ok(());
        }
        let value = service
            .load(key)
            .await
            .with_context(|| format!("load credential {key}"))?;
        self.previous.push((key.to_owned(), value));
        Ok(())
    }

    pub(super) async fn save(
        &mut self,
        service: &CredentialService,
        key: &str,
        value: &str,
    ) -> Result<()> {
        self.remember(service, key).await?;
        service
            .save(key, value)
            .await
            .with_context(|| format!("save credential {key}"))
    }

    pub(super) async fn delete(&mut self, service: &CredentialService, key: &str) -> Result<()> {
        self.remember(service, key).await?;
        service
            .delete(key)
            .await
            .with_context(|| format!("delete credential {key}"))
    }

    pub(super) async fn restore(self, service: &CredentialService) {
        for (key, value) in self.previous {
            let result = match value {
                Some(value) => service.save(&key, &value).await,
                None => service.delete(&key).await,
            };
            if let Err(error) = result {
                escode_host_control::logging::warn(
                    "source-server",
                    format!("restore credential {key}: {error}"),
                );
            }
        }
    }
}

pub(super) fn credential_keys(profile: &ConnectionProfile) -> Vec<String> {
    if profile.target.get("kind").and_then(Value::as_str) != Some("ssh") {
        return Vec::new();
    }
    ["passwordCredentialKey", "privateKeyPassphraseCredentialKey"]
        .iter()
        .filter_map(|key| {
            profile
                .target
                .get(*key)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        })
        .collect()
}

pub(super) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

impl SourceServerRegistry {
    fn settings_home(&self) -> Result<&Path> {
        if let Some(error) = &self.load_error {
            bail!("source server registry settings failed to load: {error}");
        }
        self.home
            .as_deref()
            .context("source server registry has no settings home (rpcListen home missing)")
    }

    /// 写入 settings；调用方必须持有 `write_lock`。
    async fn persist(&self, patch: Value) -> Result<()> {
        let home = self.settings_home()?;
        settings::apply_patch(home, &patch).await.map(|_| ())
    }

    /// 落盘成功后更新内存并发布 `onDidChangeDirectory`。
    fn apply_state<F>(&self, apply: F)
    where
        F: FnOnce(&mut RegistryState),
    {
        {
            let mut state = self.lock();
            apply(&mut state);
        }
        self.publish_directory_change();
    }

    /// `renameSource`：trim 后非空，更新设备显示名。
    pub async fn rename_source(&self, source_server_id: &str, display_name: &str) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let display_name = display_name.trim();
        if display_name.is_empty() {
            bail!("Source Server display name is required");
        }
        let servers = {
            let state = self.lock();
            let mut servers = state.servers.clone();
            let server = servers
                .iter_mut()
                .find(|server| server.source_server_id == source_server_id)
                .with_context(|| format!("Unknown Source Server: {source_server_id}"))?;
            server.display_name = display_name.to_owned();
            servers
        };
        self.persist(json!({ "sourceServers": servers })).await?;
        self.apply_state(|state| state.servers = servers);
        Ok(())
    }

    /// `renameProfile`：trim 后非空，更新连接显示名。
    pub async fn rename_profile(&self, profile_id: &str, display_name: &str) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let display_name = display_name.trim();
        if display_name.is_empty() {
            bail!("Connection Profile display name is required");
        }
        let profiles = {
            let state = self.lock();
            let mut profiles = state.profiles.clone();
            let profile = profiles
                .iter_mut()
                .find(|profile| profile.profile_id == profile_id)
                .with_context(|| format!("Unknown Connection Profile: {profile_id}"))?;
            profile.display_name = Some(display_name.to_owned());
            profiles
        };
        self.persist(json!({ "connectionProfiles": profiles }))
            .await?;
        self.apply_state(|state| state.profiles = profiles);
        Ok(())
    }

    /// `setPreferredProfile`：Profile 必须存在、未 retired 且属于该 Source。
    pub async fn set_preferred_profile(
        &self,
        source_server_id: &str,
        profile_id: &str,
    ) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let servers = {
            let state = self.lock();
            let profile = state
                .profiles
                .iter()
                .find(|profile| profile.profile_id == profile_id)
                .with_context(|| {
                    format!("Connection Profile {profile_id} does not belong to {source_server_id}")
                })?;
            if profile.lifecycle_state == "retired"
                || profile.source_server_id.as_deref() != Some(source_server_id)
            {
                bail!("Connection Profile {profile_id} does not belong to {source_server_id}");
            }
            let mut servers = state.servers.clone();
            let server = servers
                .iter_mut()
                .find(|server| server.source_server_id == source_server_id)
                .with_context(|| format!("Unknown Source Server: {source_server_id}"))?;
            server.preferred_profile_id = Some(profile_id.to_owned());
            servers
        };
        self.persist(json!({ "sourceServers": servers })).await?;
        self.apply_state(|state| state.servers = servers);
        Ok(())
    }

    /// `removeProfile`：删除 Profile/history、重指 preferred、删除 SSH 凭据；未知 profile 幂等。
    pub async fn remove_profile(
        &self,
        profile_id: &str,
        credentials: Option<&CredentialService>,
    ) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let current = {
            let state = self.lock();
            state
                .profiles
                .iter()
                .find(|profile| profile.profile_id == profile_id)
                .cloned()
        };
        let Some(current) = current else {
            return Ok(());
        };
        let keys = credential_keys(&current);
        if credentials.is_none() && !keys.is_empty() {
            bail!(
                "source server credentials need dataBaseDir: Main must pass dataBaseDir in rpcListen"
            );
        }
        let mut rollback = CredentialRollback::default();
        if let Some(service) = credentials {
            for key in keys {
                if let Err(error) = rollback.delete(service, &key).await {
                    rollback.restore(service).await;
                    return Err(error);
                }
            }
        }
        let (profiles, history, servers) = {
            let state = self.lock();
            let profiles: Vec<_> = state
                .profiles
                .iter()
                .filter(|profile| profile.profile_id != profile_id)
                .cloned()
                .collect();
            let history: Vec<_> = state
                .workspace_history
                .iter()
                .filter(|entry| entry.profile_id != profile_id)
                .cloned()
                .collect();
            let servers = state
                .servers
                .iter()
                .cloned()
                .map(|mut server| {
                    if server.preferred_profile_id.as_deref() == Some(profile_id) {
                        let server_id = server.source_server_id.clone();
                        server.preferred_profile_id = profiles
                            .iter()
                            .find(|candidate| {
                                candidate.lifecycle_state != "retired"
                                    && candidate.source_server_id.as_deref()
                                        == Some(server_id.as_str())
                            })
                            .map(|candidate| candidate.profile_id.clone());
                    }
                    server
                })
                .collect::<Vec<_>>();
            (profiles, history, servers)
        };
        let patch = json!({
            "connectionProfiles": profiles,
            "sourceServerWorkspaceHistory": history,
            "sourceServers": servers,
        });
        if let Err(error) = self.persist(patch).await {
            if let Some(service) = credentials {
                rollback.restore(service).await;
            }
            return Err(error);
        }
        self.apply_state(|state| {
            state.profiles = profiles;
            state.workspace_history = history;
            state.servers = servers;
            state.connection_states.remove(profile_id);
        });
        Ok(())
    }

    /// `removeSourceServer`：删除 Source、其 Profile、命中的 history 与 conversation workspace。
    pub async fn remove_source_server(
        &self,
        source_server_id: &str,
        credentials: Option<&CredentialService>,
    ) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let (profiles, servers, history) = {
            let state = self.lock();
            (
                state.profiles.clone(),
                state.servers.clone(),
                state.workspace_history.clone(),
            )
        };
        let removed: Vec<ConnectionProfile> = profiles
            .iter()
            .filter(|profile| profile.source_server_id.as_deref() == Some(source_server_id))
            .cloned()
            .collect();
        let removed_ids: BTreeSet<String> = removed
            .iter()
            .map(|profile| profile.profile_id.clone())
            .collect();
        let home = self.settings_home()?;
        let current_settings = settings::read(home).await?;
        let conversation_map = current_settings
            .get("remoteConversationWorkspaceBySourceServerId")
            .and_then(Value::as_object)
            .cloned();
        let has_conversation = conversation_map
            .as_ref()
            .is_some_and(|map| map.contains_key(source_server_id));
        let has_record = !removed.is_empty()
            || servers
                .iter()
                .any(|server| server.source_server_id == source_server_id)
            || history
                .iter()
                .any(|entry| entry.source_server_id.as_deref() == Some(source_server_id))
            || has_conversation;
        if !has_record {
            return Ok(());
        }
        let has_credential_keys = removed
            .iter()
            .any(|profile| !credential_keys(profile).is_empty());
        if credentials.is_none() && has_credential_keys {
            bail!(
                "source server credentials need dataBaseDir: Main must pass dataBaseDir in rpcListen"
            );
        }
        let mut rollback = CredentialRollback::default();
        if let Some(service) = credentials {
            for profile in &removed {
                for key in credential_keys(profile) {
                    if let Err(error) = rollback.delete(service, &key).await {
                        rollback.restore(service).await;
                        return Err(error);
                    }
                }
            }
        }
        let remaining_profiles: Vec<_> = profiles
            .into_iter()
            .filter(|profile| profile.source_server_id.as_deref() != Some(source_server_id))
            .collect();
        let remaining_servers: Vec<_> = servers
            .into_iter()
            .filter(|server| server.source_server_id != source_server_id)
            .collect();
        let remaining_history: Vec<_> = history
            .into_iter()
            .filter(|entry| {
                entry.source_server_id.as_deref() != Some(source_server_id)
                    && !removed_ids.contains(&entry.profile_id)
            })
            .collect();
        let mut patch = Map::new();
        patch.insert(
            "connectionProfiles".to_owned(),
            serde_json::to_value(&remaining_profiles)?,
        );
        patch.insert(
            "sourceServers".to_owned(),
            serde_json::to_value(&remaining_servers)?,
        );
        patch.insert(
            "sourceServerWorkspaceHistory".to_owned(),
            serde_json::to_value(&remaining_history)?,
        );
        if let Some(mut map) = conversation_map {
            map.remove(source_server_id);
            patch.insert(
                "remoteConversationWorkspaceBySourceServerId".to_owned(),
                Value::Object(map),
            );
        }
        if let Err(error) = self.persist(Value::Object(patch)).await {
            if let Some(service) = credentials {
                rollback.restore(service).await;
            }
            return Err(error);
        }
        self.apply_state(|state| {
            state.profiles = remaining_profiles;
            state.servers = remaining_servers;
            state.workspace_history = remaining_history;
            for profile_id in &removed_ids {
                state.connection_states.remove(profile_id);
            }
        });
        Ok(())
    }
}

#[cfg(test)]
#[path = "source_server_writes_tests.rs"]
mod tests;
