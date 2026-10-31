//! Connection Profile 首连匹配/创建（S5-a，docs/specs/source-server-remote-rust-host.md §12.2）。
//!
//! Main 不直接写 settings：无 Profile 的目标先交 Host 归一化、按 transport + 目标身份匹配，
//! 必要时创建 Profile 并持久化 SSH 凭据引用。`sourceServerId` 由随后的连接握手回填。

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::credential::CredentialService;

use super::super::{ConnectionProfile, SourceServerRegistry};
use super::target_normalize::{kind_of, normalize_target};
use super::{CredentialRollback, now_ms};

impl SourceServerRegistry {
    /// 按远端目标建立或复用 active Connection Profile。
    ///
    /// 目标身份与 TS `profileTargetIdentityKey` 一致：SSH 忽略 alias/安装模式/资源包，
    /// WSL 只比较 distro/user，Docker 只比较 container。明文密码/口令只进凭据服务。
    pub async fn ensure_profile(
        &self,
        target: &Value,
        credentials: Option<&CredentialService>,
    ) -> Result<ConnectionProfile> {
        let _guard = self.write_lock.lock().await;
        let normalized = normalize_target(target)?;
        let transport = kind_of(&normalized).to_owned();
        let identity_key = profile_target_identity_key(&normalized)?;
        let existing = {
            let state = self.lock();
            state
                .profiles
                .iter()
                .filter(|profile| profile.lifecycle_state != "retired")
                .filter(|profile| profile.transport == transport)
                .find(|profile| {
                    profile_target_identity_key(&profile.target)
                        .is_ok_and(|key| key == identity_key)
                })
                .cloned()
        };

        let profile_id = existing.as_ref().map_or_else(
            || Uuid::new_v4().to_string(),
            |profile| profile.profile_id.clone(),
        );
        let mut rollback = CredentialRollback::default();
        let next_target = match persist_credentials(
            &profile_id,
            target,
            normalized,
            existing.as_ref().map(|profile| &profile.target),
            credentials,
            &mut rollback,
        )
        .await
        {
            Ok(target) => target,
            Err(error) => {
                if let Some(service) = credentials {
                    rollback.restore(service).await;
                }
                return Err(error);
            }
        };

        let Some(mut profile) = existing else {
            let profile = ConnectionProfile {
                profile_id,
                transport,
                display_name: default_display_name(&next_target),
                target: next_target,
                source_server_id: None,
                last_connected_at: None,
                last_connect_error: None,
                created_at: now_ms(),
                lifecycle_state: "active".to_owned(),
                retired_reason: None,
                retired_at: None,
            };
            let profiles = {
                let state = self.lock();
                let mut profiles = state.profiles.clone();
                profiles.push(profile.clone());
                profiles
            };
            if let Err(error) = self
                .persist(serde_json::json!({ "connectionProfiles": &profiles }))
                .await
            {
                if let Some(service) = credentials {
                    rollback.restore(service).await;
                }
                return Err(error);
            }
            self.apply_state(|state| state.profiles = profiles);
            return Ok(profile);
        };

        if profile.target == next_target {
            return Ok(profile);
        }
        profile.target = next_target;
        let profiles = {
            let state = self.lock();
            let mut profiles = state.profiles.clone();
            let index = profiles
                .iter()
                .position(|candidate| candidate.profile_id == profile.profile_id)
                .context("Connection Profile disappeared during ensureProfile")?;
            profiles[index] = profile.clone();
            profiles
        };
        if let Err(error) = self
            .persist(serde_json::json!({ "connectionProfiles": &profiles }))
            .await
        {
            if let Some(service) = credentials {
                rollback.restore(service).await;
            }
            return Err(error);
        }
        self.apply_state(|state| state.profiles = profiles);
        Ok(profile)
    }
}

fn profile_target_identity_key(target: &Value) -> Result<String> {
    let object = target
        .as_object()
        .context("Connection target must be an object")?;
    let key = match kind_of(target) {
        "ssh" => serde_json::json!({
            "kind": "ssh",
            "host": string_field(object, "host").map(|value| value.trim().to_lowercase()).unwrap_or_default(),
            "port": object.get("port").and_then(Value::as_u64).unwrap_or(22),
            "username": string_field(object, "username").map(str::trim).unwrap_or_default(),
            "privateKeyPath": string_field(object, "privateKeyPath").map(str::trim).unwrap_or_default(),
        }),
        "wsl" => serde_json::json!({
            "kind": "wsl",
            "distro": string_field(object, "distro").map(str::trim).unwrap_or_default(),
            "user": string_field(object, "user").map(str::trim).unwrap_or_default(),
        }),
        "docker" => serde_json::json!({
            "kind": "docker",
            "container": string_field(object, "container").map(str::trim).unwrap_or_default(),
        }),
        // spec §12.6.2：URL 原样作为身份键，不做 http/ws 协议互转（同一 host 的两种入口仍是两个目标）。
        "server" => serde_json::json!({
            "kind": "server",
            "url": string_field(object, "url").map(str::trim).unwrap_or_default(),
        }),
        other => bail!("Unsupported connection transport: {other}"),
    };
    Ok(key.to_string())
}

async fn persist_credentials(
    profile_id: &str,
    input: &Value,
    mut target: Value,
    current: Option<&Value>,
    credentials: Option<&CredentialService>,
    rollback: &mut CredentialRollback,
) -> Result<Value> {
    if kind_of(&target) == "server" {
        return persist_server_token(profile_id, input, target, current, credentials, rollback)
            .await;
    }
    if kind_of(&target) != "ssh" {
        return Ok(target);
    }
    let password = string_field(
        input
            .as_object()
            .context("Connection target must be an object")?,
        "password",
    )
    .map(str::trim)
    .filter(|value| !value.is_empty());
    let passphrase = string_field(
        input
            .as_object()
            .context("Connection target must be an object")?,
        "privateKeyPassphrase",
    )
    .map(str::trim)
    .filter(|value| !value.is_empty());
    if password.is_none() && passphrase.is_none() {
        preserve_credential_keys(
            &mut target,
            current,
            &["passwordCredentialKey", "privateKeyPassphraseCredentialKey"],
        )?;
        return Ok(target);
    }
    let service = credentials.context(
        "source server credentials need dataBaseDir: Main must pass dataBaseDir in rpcListen",
    )?;
    let current_password_key = current
        .and_then(|target| string_field(target.as_object()?, "passwordCredentialKey"))
        .map(str::to_owned);
    let current_passphrase_key = current
        .and_then(|target| string_field(target.as_object()?, "privateKeyPassphraseCredentialKey"))
        .map(str::to_owned);
    let object = target
        .as_object_mut()
        .context("SSH Connection Profile target must be an object")?;

    if let Some(password) = password {
        let key = current_password_key
            .clone()
            .unwrap_or_else(|| format!("connection-profile:{profile_id}:password"));
        rollback.save(service, &key, password).await?;
        object.insert("passwordCredentialKey".to_owned(), Value::from(key));
    }
    if let Some(passphrase) = passphrase {
        let key = current_passphrase_key
            .clone()
            .unwrap_or_else(|| format!("connection-profile:{profile_id}:private-key-passphrase"));
        rollback.save(service, &key, passphrase).await?;
        object.insert(
            "privateKeyPassphraseCredentialKey".to_owned(),
            Value::from(key),
        );
    }
    if passphrase.is_none() {
        object.remove("privateKeyPassphraseCredentialKey");
        if let Some(key) = current_passphrase_key {
            rollback.delete(service, &key).await?;
        }
    }
    if password.is_none() {
        object.remove("passwordCredentialKey");
        if let Some(key) = current_password_key {
            rollback.delete(service, &key).await?;
        }
    }
    Ok(target)
}

/// server 直连 token：只落凭据服务，settings 保存 `tokenCredentialKey` 引用。
///
/// 本次未携带 token（重启恢复/仅重连）时必须保留旧引用，否则已保存的认证会丢失。
async fn persist_server_token(
    profile_id: &str,
    input: &Value,
    mut target: Value,
    current: Option<&Value>,
    credentials: Option<&CredentialService>,
    rollback: &mut CredentialRollback,
) -> Result<Value> {
    let token = string_field(
        input
            .as_object()
            .context("Connection target must be an object")?,
        "token",
    )
    .map(str::trim)
    .filter(|value| !value.is_empty());
    let Some(token) = token else {
        preserve_credential_keys(&mut target, current, &["tokenCredentialKey"])?;
        return Ok(target);
    };
    let service = credentials.context(
        "source server credentials need dataBaseDir: Main must pass dataBaseDir in rpcListen",
    )?;
    let key = current
        .and_then(|current| string_field(current.as_object()?, "tokenCredentialKey"))
        .map(str::to_owned)
        .unwrap_or_else(|| format!("connection-profile:{profile_id}:token"));
    rollback.save(service, &key, token).await?;
    target
        .as_object_mut()
        .context("Server Connection Profile target must be an object")?
        .insert("tokenCredentialKey".to_owned(), Value::from(key));
    Ok(target)
}

fn preserve_credential_keys(
    target: &mut Value,
    current: Option<&Value>,
    fields: &[&str],
) -> Result<()> {
    let Some(current) = current.and_then(Value::as_object) else {
        return Ok(());
    };
    let object = target
        .as_object_mut()
        .context("Connection Profile target must be an object")?;
    for &field in fields {
        if let Some(value) = current
            .get(field)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            object.insert(field.to_owned(), Value::from(value));
        }
    }
    Ok(())
}

fn default_display_name(target: &Value) -> Option<String> {
    let object = target.as_object()?;
    match kind_of(target) {
        "ssh" => {
            let alias = string_field(object, "sshConfigAlias")
                .map(str::trim)
                .filter(|value| !value.is_empty());
            alias
                .or_else(|| string_field(object, "host").map(str::trim))
                .map(str::to_owned)
        }
        "wsl" => Some(
            string_field(object, "distro")
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("WSL")
                .to_owned(),
        ),
        "docker" => string_field(object, "container").map(str::to_owned),
        "server" => string_field(object, "url").map(str::to_owned),
        _ => None,
    }
}

fn string_field<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str)
}

#[cfg(test)]
#[path = "source_server_profile_ensure_tests.rs"]
mod tests;
