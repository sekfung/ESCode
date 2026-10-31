//! `updateProfile` 写面（spec §9.8）：原位更新 Profile/target，并在 SSH 认证方式切换时
//! 轮换凭据。拆出以守住单文件有效行数上限。

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};

use crate::credential::CredentialService;

use super::super::{ConnectionProfile, SourceServerRecord, SourceServerRegistry};
use super::CredentialRollback;
use super::target_normalize::{kind_of, normalize_target};

/// `updateProfile` 的输入（TS `UpdateConnectionProfileInput`）。
#[derive(Clone, Debug)]
pub struct UpdateProfileInput {
    pub profile_id: String,
    pub profile_display_name: String,
    pub source_display_name: Option<String>,
    pub target: Value,
    pub ssh_auth_method: Option<String>,
}

impl UpdateProfileInput {
    pub fn from_json(value: &Value) -> Result<Self> {
        let object = value
            .as_object()
            .context("updateProfile requires an input object")?;
        let profile_id = object
            .get("profileId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .context("updateProfile requires profileId")?
            .to_owned();
        let profile_display_name = object
            .get("profileDisplayName")
            .and_then(Value::as_str)
            .context("updateProfile requires profileDisplayName")?
            .to_owned();
        let source_display_name =
            optional_string(object.get("sourceDisplayName"), "sourceDisplayName")?;
        let target = object
            .get("target")
            .cloned()
            .context("updateProfile requires target")?;
        let ssh_auth_method = optional_string(object.get("sshAuthMethod"), "sshAuthMethod")?;
        Ok(Self {
            profile_id,
            profile_display_name,
            source_display_name,
            target,
            ssh_auth_method,
        })
    }
}

fn optional_string(value: Option<&Value>, field: &str) -> Result<Option<String>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(|value| Some(value.to_owned()))
            .with_context(|| format!("{field} must be a string")),
    }
}

impl SourceServerRegistry {
    /// `updateProfile`：原位更新显示名与目标，SSH 认证方式切换时轮换凭据。
    pub async fn update_profile(
        &self,
        input: UpdateProfileInput,
        credentials: Option<&CredentialService>,
    ) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let display_name = input.profile_display_name.trim().to_owned();
        if display_name.is_empty() {
            bail!("Connection Profile display name is required");
        }
        let source_display_name = match input.source_display_name.as_deref() {
            None => None,
            Some(value) => {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    bail!("Source Server display name is required");
                }
                Some(trimmed.to_owned())
            }
        };
        let current = {
            let state = self.lock();
            state
                .profiles
                .iter()
                .find(|profile| profile.profile_id == input.profile_id)
                .cloned()
                .with_context(|| format!("Unknown Connection Profile: {}", input.profile_id))?
        };
        if current.lifecycle_state == "retired" {
            bail!("Connection Profile is retired: {}", input.profile_id);
        }
        let mut next_target = normalize_target(&input.target)?;
        if current.transport != kind_of(&next_target) {
            bail!("Connection Profile transport cannot be changed");
        }

        let mut rollback = CredentialRollback::default();
        let outcome = self
            .prepare_profile_update(
                &input,
                &current,
                &display_name,
                source_display_name.as_deref(),
                &mut next_target,
                &mut rollback,
                credentials,
            )
            .await;
        let (profiles, servers) = match outcome {
            Ok(value) => value,
            Err(error) => {
                if let Some(service) = credentials {
                    rollback.restore(service).await;
                }
                return Err(error);
            }
        };
        let mut patch = Map::new();
        patch.insert(
            "connectionProfiles".to_owned(),
            serde_json::to_value(&profiles)?,
        );
        if let Some(servers) = &servers {
            patch.insert("sourceServers".to_owned(), serde_json::to_value(servers)?);
        }
        if let Err(error) = self.persist(Value::Object(patch)).await {
            if let Some(service) = credentials {
                rollback.restore(service).await;
            }
            return Err(error);
        }
        self.apply_state(|state| {
            state.profiles = profiles;
            if let Some(servers) = servers {
                state.servers = servers;
            }
        });
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn prepare_profile_update(
        &self,
        input: &UpdateProfileInput,
        current: &ConnectionProfile,
        display_name: &str,
        source_display_name: Option<&str>,
        next_target: &mut Value,
        rollback: &mut CredentialRollback,
        credentials: Option<&CredentialService>,
    ) -> Result<(Vec<ConnectionProfile>, Option<Vec<SourceServerRecord>>)> {
        if current.transport == "ssh" {
            let service = credentials.context(
                "source server credentials need dataBaseDir: Main must pass dataBaseDir in rpcListen",
            )?;
            rotate_ssh_auth(service, rollback, current, input, next_target).await?;
        }
        let mut profile = current.clone();
        profile.display_name = Some(display_name.to_owned());
        profile.target = next_target.clone();
        profile.last_connect_error = None;
        let mut profiles = {
            let state = self.lock();
            state.profiles.clone()
        };
        let index = profiles
            .iter()
            .position(|candidate| candidate.profile_id == current.profile_id)
            .with_context(|| format!("Unknown Connection Profile: {}", input.profile_id))?;
        profiles[index] = profile;
        let servers = match source_display_name {
            None => None,
            Some(name) => {
                let source_server_id = current
                    .source_server_id
                    .clone()
                    .context("Unidentified Connection Profile cannot rename a Source Server")?;
                let mut servers = {
                    let state = self.lock();
                    state.servers.clone()
                };
                let server = servers
                    .iter_mut()
                    .find(|server| server.source_server_id == source_server_id)
                    .with_context(|| format!("Unknown Source Server: {source_server_id}"))?;
                server.display_name = name.to_owned();
                Some(servers)
            }
        };
        Ok((profiles, servers))
    }
}

fn target_string<'a>(target: &'a Value, key: &str) -> Option<&'a str> {
    target
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

/// SSH 认证方式切换：密码 ↔ 私钥口令。输入里的明文只进凭据服务，不落 settings。
async fn rotate_ssh_auth(
    service: &CredentialService,
    rollback: &mut CredentialRollback,
    current: &ConnectionProfile,
    input: &UpdateProfileInput,
    target: &mut Value,
) -> Result<()> {
    let method = input
        .ssh_auth_method
        .as_deref()
        .context("SSH authentication method is required")?;
    let current_password_key =
        target_string(&current.target, "passwordCredentialKey").map(str::to_owned);
    let current_passphrase_key =
        target_string(&current.target, "privateKeyPassphraseCredentialKey").map(str::to_owned);
    let entered_password = input
        .target
        .get("password")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    let entered_passphrase = input
        .target
        .get("privateKeyPassphrase")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    let current_private_key_path = target_string(&current.target, "privateKeyPath");
    let next_private_key_path = target_string(target, "privateKeyPath");

    match method {
        "password" => {
            let key = current_password_key
                .clone()
                .unwrap_or_else(|| format!("connection-profile:{}:password", current.profile_id));
            let saved = match &current_password_key {
                Some(current_key) => service
                    .load(current_key)
                    .await
                    .with_context(|| format!("load credential {current_key}"))?,
                None => None,
            };
            let has_saved = saved
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty());
            if entered_password.is_empty() && !has_saved {
                bail!("SSH password is required");
            }
            if !entered_password.is_empty() {
                rollback.save(service, &key, entered_password).await?;
            }
            if let Some(old_key) = &current_passphrase_key {
                rollback.delete(service, old_key).await?;
            }
            let object = target
                .as_object_mut()
                .context("SSH target must be an object")?;
            object.remove("privateKeyPath");
            object.insert("passwordCredentialKey".to_owned(), Value::from(key));
        }
        "privateKey" => {
            let private_key_path = next_private_key_path
                .map(str::trim)
                .unwrap_or_default()
                .to_owned();
            if private_key_path.is_empty() {
                bail!("SSH private key path is required");
            }
            if let Some(old_key) = &current_password_key {
                rollback.delete(service, old_key).await?;
            }
            let next_key = if !entered_passphrase.is_empty() {
                let key = current_passphrase_key.clone().unwrap_or_else(|| {
                    format!(
                        "connection-profile:{}:private-key-passphrase",
                        current.profile_id
                    )
                });
                rollback.save(service, &key, entered_passphrase).await?;
                Some(key)
            } else if current_passphrase_key.is_some()
                && current_private_key_path == next_private_key_path
            {
                current_passphrase_key.clone()
            } else {
                if let Some(old_key) = &current_passphrase_key {
                    rollback.delete(service, old_key).await?;
                }
                None
            };
            let object = target
                .as_object_mut()
                .context("SSH target must be an object")?;
            match next_key {
                Some(key) => {
                    object.insert(
                        "privateKeyPassphraseCredentialKey".to_owned(),
                        Value::from(key),
                    );
                }
                None => {
                    object.remove("privateKeyPassphraseCredentialKey");
                }
            }
        }
        other => bail!("SSH authentication method is invalid: {other}"),
    }
    Ok(())
}

#[cfg(test)]
#[path = "source_server_profile_update_tests.rs"]
mod tests;
