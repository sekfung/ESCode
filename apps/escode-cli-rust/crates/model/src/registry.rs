//! Existing App provider files, resolved once per configuration revision.
use super::registry_rules::{resolve, validate_account};
use crate::contract::{ModelIdentity, ModelPort, ModelRegistry};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, RwLock},
};

pub(super) type ModelKey = (String, String, String);
#[derive(Default)]
pub(super) struct Snapshot {
    pub(super) signature: Vec<u8>,
    pub(super) models: BTreeMap<ModelKey, Arc<dyn ModelPort>>,
    pub(super) catalog: Vec<Value>,
    /// `ListModels` 的目录面（TS `createModelCatalogPort`）：与 picker 面同一份注册表、另一套字段
    /// （providerLabel 保持“没取过名就缺席”，档位表与默认档位同 picker 的规则）。
    pub(super) model_catalog: Vec<Value>,
    pub(super) options: Vec<Value>,
    pub(super) default: Option<ModelIdentity>,
}
pub struct Registry {
    builtin: PathBuf,
    personal: PathBuf,
    account: tokio::sync::Mutex<Option<Value>>,
    snapshot: RwLock<Snapshot>,
    pool: Arc<tokio::sync::OnceCell<reqwest::Client>>,
    /// 文件真读闸门（P1 #7）：指纹未变就跳过两份配置的读取与 SHA256；
    /// 与 `account` 同一串行段内更新（refresh 是唯一调用者路径）。
    gate: tokio::sync::Mutex<escode_cli_host::file_stamp::ChangeGate>,
}
impl Registry {
    /// 读取的是环境实例的 ambient env 快照（`core_api::env`），不是进程 env 直读：
    /// 多实例（embed/测试）各自看到自己的配置文件注入。
    pub async fn from_env() -> Result<Option<Arc<Self>>> {
        let builtin = escode_cli_core_api::env::var_os("ESCODE_BUILTIN_PROVIDER_CONFIG_FILE");
        let personal = escode_cli_core_api::env::var_os("ESCODE_PERSONAL_PROVIDER_CONFIG_FILE");
        if builtin.is_none() && personal.is_none() {
            return Ok(None);
        }
        let registry = Arc::new(Self::new(
            builtin.context("Builtin config required")?.into(),
            personal.context("Personal config required")?.into(),
        ));
        registry.refresh(None).await?;
        Ok(Some(registry))
    }
    pub fn new(builtin: PathBuf, personal: PathBuf) -> Self {
        Self {
            builtin,
            personal,
            account: Default::default(),
            snapshot: Default::default(),
            pool: Default::default(),
            // 兜底全读间隔 60s：Windows 无 inode/ctime，且同粒度原地写可能骗过指纹
            // （docs/specs/rust-unified-host.md P1 #7）。
            gate: tokio::sync::Mutex::new(escode_cli_host::file_stamp::ChangeGate::new(
                std::time::Duration::from_secs(60),
            )),
        }
    }
    /// 锁中毒继续用：快照是整块替换的自包含值，中毒后仍是最后一次成功写入的完整状态
    /// （P0#4：读路径不因写路径的 panic 把整个 runtime 打崩）。
    fn snapshot(&self) -> std::sync::RwLockReadGuard<'_, Snapshot> {
        self.snapshot
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
#[async_trait::async_trait]
impl ModelRegistry for Registry {
    async fn received_account(&self) -> Option<Value> {
        self.account.lock().await.clone()
    }
    fn model_options(&self) -> Vec<Value> {
        self.snapshot().options.clone()
    }
    fn catalog(&self) -> Vec<Value> {
        self.snapshot().catalog.clone()
    }
    fn model_catalog(&self) -> Vec<Value> {
        self.snapshot().model_catalog.clone()
    }
    fn default_selection(&self) -> Option<ModelIdentity> {
        self.snapshot().default.clone()
    }
    fn resolve(&self, s: &ModelIdentity) -> Result<Arc<dyn ModelPort>> {
        self.snapshot()
            .models
            .get(&(
                s.provider_id.clone(),
                s.model_id.clone(),
                s.reasoning_level.clone(),
            ))
            .cloned()
            .context("Selected model is unavailable")
    }
    async fn refresh(&self, update: Option<Value>) -> Result<bool> {
        // 配置刷新共用一个串行入口；迟到的文件读取不能覆盖新账号事实。
        let mut account = self.account.lock().await;
        let account_updated = update.is_some();
        if let Some(next) = update {
            validate_account(&next)?;
            *account = Some(next);
        }
        // 文件指纹在读前取：读到一半发生的变化会体现在下一轮指纹里（读取失败不记录，
        // 下一轮自然真读——行为等同于没有闸门）。
        let stamps = vec![
            escode_cli_host::file_stamp::FileStamp::of(&self.builtin).await,
            escode_cli_host::file_stamp::FileStamp::of(&self.personal).await,
        ];
        let now = std::time::Instant::now();
        {
            let gate = self.gate.lock().await;
            // 账号事实变化（update）必须重算签名（签名含账号），绕过闸门。
            if !account_updated && !gate.should_read(&stamps, now) {
                return Ok(false);
            }
        }
        let builtin = tokio::fs::read(&self.builtin)
            .await
            .context("Cannot read builtin provider config")?;
        let personal = match tokio::fs::read(&self.personal).await {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                b"{\"schemaVersion\":1,\"config\":{}}".to_vec()
            }
            Err(_) => anyhow::bail!("Cannot read personal provider config"),
        };
        ensure!(
            builtin.len() <= 8 * 1024 * 1024 && personal.len() <= 8 * 1024 * 1024,
            "Provider config exceeds limit"
        );
        let mut hash = Sha256::new();
        hash.update(&builtin);
        hash.update(&personal);
        hash.update(serde_json::to_vec(&*account)?);
        let signature = hash.finalize().to_vec();
        self.gate.lock().await.record(stamps.clone(), now);
        if self.snapshot().signature == signature {
            return Ok(false);
        }
        let builtin: Value =
            serde_json::from_slice(&builtin).context("Invalid builtin provider config")?;
        let personal: Value =
            serde_json::from_slice(&personal).context("Invalid personal provider config")?;
        ensure!(
            builtin["schemaVersion"] == 1 && personal["schemaVersion"] == 1,
            "Unsupported provider schema"
        );
        let path = std::path::absolute(&self.builtin)?;
        let source = format!("{:x}", Sha256::digest(path.to_string_lossy().as_bytes()));
        let revision = format!(
            "escode-builtin:{}:{source}",
            builtin["revision"]
                .as_u64()
                .context("Invalid builtin revision")?
        );
        if let Some(account) = &*account
            && account["basedOnESCodeBuiltinRevision"] != revision
        {
            return Ok(false);
        }
        let pool = self.pool.clone();
        let account_value = account.clone().unwrap_or_else(|| json!({}));
        // 阻塞解析放到受管阻塞线程：登记在册、panic 上报，并把实例 env 快照重建到该线程。
        let mut next = escode_cli_core_api::scope::Spawner::current()
            .spawn_blocking("registry-resolve", move || {
                resolve(
                    &builtin["config"],
                    &personal["config"],
                    &account_value,
                    pool,
                )
            })
            .await??;
        next.signature = signature;
        *self
            .snapshot
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = next;
        Ok(true)
    }
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod registry_tests;
