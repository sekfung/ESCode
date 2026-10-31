//! Provider Provisioning 的本地事实源（S4-g）：与 TS `createProviderProvisioningSource` 同契约。

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

use super::{PROVIDER_PROVISIONING_OAUTH_CREDENTIAL_KEYS, ProviderProvisioningSource};
use crate::credential::CredentialService;
use crate::provider::ProviderService;
use crate::settings;

/// Local Environment 的只读事实源。
pub struct LocalProviderProvisioningSource {
    provider: Arc<ProviderService>,
    home: PathBuf,
    credentials: Arc<CredentialService>,
}

impl LocalProviderProvisioningSource {
    pub fn new(
        provider: Arc<ProviderService>,
        home: PathBuf,
        credentials: Arc<CredentialService>,
    ) -> Self {
        Self {
            provider,
            home,
            credentials,
        }
    }
}

#[async_trait::async_trait]
impl ProviderProvisioningSource for LocalProviderProvisioningSource {
    async fn read(&self, sync_id: &str) -> Result<Value> {
        // Personal、设置与凭据是同一份 Envelope 的三个事实；读取顺序不影响目标端事务。
        let personal_config = self.provider.personal_config_for_provisioning().await?;
        let app_settings = settings::read(&self.home).await?;
        let account_settings = account_settings(&app_settings)?;
        let credentials = self
            .credentials
            .provisioning_credentials(PROVIDER_PROVISIONING_OAUTH_CREDENTIAL_KEYS)
            .await
            .map_err(|error| anyhow!("{error}"))?;
        let credentials: Vec<Value> = credentials
            .into_iter()
            .map(|(key, value)| json!({"scope": "oauth-session", "key": key, "value": value}))
            .collect();
        Ok(json!({
            "schemaVersion": 2,
            "syncId": sync_id,
            "personalConfig": personal_config,
            "accountSettings": account_settings,
            "credentials": credentials,
        }))
    }
}

fn account_settings(settings: &Value) -> Result<Value> {
    let domain = match settings.get("providerFamilyDomain") {
        None | Some(Value::Null) => Value::Null,
        Some(Value::String(value)) if value == "zai" || value == "bigmodel" => {
            Value::String(value.clone())
        }
        Some(other) => bail!("providerFamilyDomain 不受支持: {other}"),
    };
    let selections = settings
        .get("providerFamilyConnectionSelections")
        .cloned()
        .unwrap_or_else(|| json!({}));
    validate_connection_selections(&selections)?;
    Ok(json!({
        "providerFamilyDomain": domain,
        "providerFamilyConnectionSelections": selections,
    }))
}

fn validate_connection_selections(value: &Value) -> Result<()> {
    let Some(map) = value.as_object() else {
        bail!("providerFamilyConnectionSelections 必须是对象");
    };
    for (family, selection) in map {
        if family != "zai" && family != "bigmodel" {
            bail!("providerFamilyConnectionSelections 含未知 Family: {family}");
        }
        let Some(selection) = selection.as_object() else {
            bail!("providerFamilyConnectionSelections.{family} 必须是对象");
        };
        let kind = selection
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("providerFamilyConnectionSelections.{family}.kind 缺失"))?;
        let allowed: &[&str] = match kind {
            "start-plan" | "individual-coding-plan" => &["kind"],
            "team-coding-plan" => &["kind", "productId", "organizationId", "projectId"],
            _ => bail!("providerFamilyConnectionSelections.{family}.kind 不受支持: {kind}"),
        };
        if let Some(extra) = selection
            .keys()
            .find(|key| !allowed.contains(&key.as_str()))
        {
            bail!("providerFamilyConnectionSelections.{family} 含未知字段 {extra}");
        }
        if kind == "team-coding-plan" {
            for key in ["productId", "organizationId", "projectId"] {
                let value = selection
                    .get(key)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                if value.is_none() {
                    bail!("providerFamilyConnectionSelections.{family}.{key} 不能为空");
                }
            }
        }
    }
    Ok(())
}
