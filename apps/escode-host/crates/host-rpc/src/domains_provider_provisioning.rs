//! Provider Provisioning 组合根辅助（S4-g）：本地事实源、Personal 写入监听与触发闭包。

use std::path::Path;
use std::sync::Arc;

use escode_host_services::credential::CredentialService;
use escode_host_services::provider::ProviderService;
use escode_host_services::source_server::SourceServerRegistry;
use escode_host_services::source_server_connections::SourceServerConnections;
use escode_host_services::source_server_provider_provisioning::{
    LocalProviderProvisioningSource, PROVIDER_PROVISIONING_TIMEOUT, ProvisioningTrigger,
    SourceServerProviderProvisioning,
};

pub(super) struct SourceServerAssembly {
    pub registry: Arc<SourceServerRegistry>,
    pub connections: Arc<SourceServerConnections>,
    /// Source Server Profile 写面的凭据 owner（`dataBaseDir` 缺失时为 `None`）。
    pub credentials: Option<Arc<CredentialService>>,
    pub settings_trigger: Option<Arc<dyn Fn() + Send + Sync>>,
    pub credential_trigger: Option<Arc<dyn Fn() + Send + Sync>>,
    provisioning: Option<Arc<SourceServerProviderProvisioning>>,
}

impl SourceServerAssembly {
    pub fn spawn_personal_change_listener(&self, provider_service: Option<&Arc<ProviderService>>) {
        spawn_personal_change_listener(provider_service, self.provisioning(), &self.connections);
    }

    pub fn start_recovery(&self) {
        crate::domains_source_server::start_source_server_recovery(
            &self.registry,
            &self.connections,
        );
    }

    fn provisioning(&self) -> Option<&Arc<SourceServerProviderProvisioning>> {
        self.provisioning.as_ref()
    }
}

/// 连接 registry / 连接管理器和 Provisioning 执行器在同一处装配，避免三份 owner 各自初始化。
pub(super) fn assemble_source_servers(
    home: &Path,
    data_base_dir: Option<&Path>,
    provider_service: Option<&Arc<ProviderService>>,
    host: Option<crate::domains_source_server::HostEnvironment>,
) -> SourceServerAssembly {
    let provisioning = build(provider_service, home, data_base_dir);
    let credentials = data_base_dir.map(|base| {
        Arc::new(CredentialService::new(
            CredentialService::path(base),
            &escode_cli_host::credential_cipher::credential_secret(),
        ))
    });
    let registry = Arc::new(SourceServerRegistry::load(home));
    let connections = Arc::new(SourceServerConnections::new(
        Arc::clone(&registry),
        crate::domains_source_server::default_connections_options(data_base_dir, host)
            .with_provisioning(provisioning.clone())
            .with_preferences_home(Some(home.to_path_buf())),
    ));
    let settings_trigger = compose_trigger(
        provisioning.as_ref(),
        &connections,
        ProvisioningTrigger::AccountSettings,
    );
    let credential_trigger = compose_trigger(
        provisioning.as_ref(),
        &connections,
        ProvisioningTrigger::Credential,
    );
    SourceServerAssembly {
        registry,
        connections,
        credentials,
        settings_trigger,
        credential_trigger,
        provisioning,
    }
}

fn build(
    provider_service: Option<&Arc<ProviderService>>,
    home: &Path,
    data_base_dir: Option<&Path>,
) -> Option<Arc<SourceServerProviderProvisioning>> {
    let provider = provider_service?;
    let base = data_base_dir?;
    let credentials = Arc::new(escode_host_services::credential::CredentialService::new(
        escode_host_services::credential::CredentialService::path(base),
        &escode_cli_host::credential_cipher::credential_secret(),
    ));
    let source = Arc::new(LocalProviderProvisioningSource::new(
        Arc::clone(provider),
        home.to_path_buf(),
        credentials,
    ));
    Some(Arc::new(SourceServerProviderProvisioning::new(
        source,
        PROVIDER_PROVISIONING_TIMEOUT,
    )))
}

pub(super) fn spawn_personal_change_listener(
    provider_service: Option<&Arc<ProviderService>>,
    provisioning: Option<&Arc<SourceServerProviderProvisioning>>,
    connections: &Arc<SourceServerConnections>,
) {
    let (Some(provider), Some(_)) = (provider_service, provisioning) else {
        return;
    };
    let mut changes = provider.watch_personal_changes();
    let connections = Arc::clone(connections);
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(async move {
            while changes.changed().await.is_ok() {
                connections.notify_provider_provisioning(ProvisioningTrigger::PersonalConfig);
            }
        });
    }
}

/// 只有 Provider 账号窄设置变更才推进远端 Provisioning；其余设置键不触发。
pub(super) fn notify_if_provisioning_key(
    trigger: Option<Arc<dyn Fn() + Send + Sync>>,
    keys: &[String],
) {
    if let Some(trigger) = trigger
        && keys.iter().any(|key| affects_provisioning(key))
    {
        trigger();
    }
}

fn affects_provisioning(key: &str) -> bool {
    matches!(
        key,
        "providerFamilyDomain" | "providerFamilyConnectionSelections"
    )
}

pub(super) fn compose_trigger(
    provisioning: Option<&Arc<SourceServerProviderProvisioning>>,
    connections: &Arc<SourceServerConnections>,
    trigger: ProvisioningTrigger,
) -> Option<Arc<dyn Fn() + Send + Sync>> {
    provisioning.as_ref()?;
    let connections = Arc::clone(connections);
    Some(Arc::new(move || {
        connections.notify_provider_provisioning(trigger);
    }))
}
