//! 连接管理器配置与连接意图（S3-a/S4-e）。

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use super::{
    DEFAULT_CLIENT_ID, INITIALIZE_TIMEOUT, InstallOptions, TransportFuture, TransportResolver,
};
use crate::credential::CredentialService;
use crate::source_server::ServerIdentity;
use crate::source_server_compatibility::CompatibilityManifest;
use crate::source_server_exec::RemoteRunner;
use crate::source_server_provider_provisioning::SourceServerProviderProvisioning;

/// 远端 server-info probe（S4-j）。测试注入固定身份；生产走 `probe_server_identity`。
pub type IdentityProbe = Arc<
    dyn Fn(
            Arc<dyn RemoteRunner>,
        )
            -> Pin<Box<dyn std::future::Future<Output = anyhow::Result<ServerIdentity>> + Send>>
        + Send
        + Sync,
>;

pub struct ConnectionsOptions {
    pub client_version: String,
    pub client_id: String,
    pub initialize_timeout: Duration,
    /// 本地兼容清单（productVersion = Main 注入的 app 版本）。
    pub compatibility: CompatibilityManifest,
    pub resolve_transport: TransportResolver,
    /// 首次安装配置；Main 未交付 remote assets 时为 None（保持旧行为，只启动 server）。
    pub install: Option<InstallOptions>,
    /// 断线自动恢复的延迟档位（测试注入毫秒级，生产 2s/4s/8s）。
    pub recovery_delays: [Duration; 3],
    /// 启动自动连接单次上限（生产 10s；测试可缩短）。
    pub startup_timeout: Duration,
    /// Provider Provisioning（S4-g）：Host 私有执行器；None = 该 Host 尚未装配本地 Provider 事实。
    pub provisioning: Option<Arc<SourceServerProviderProvisioning>>,
    /// 远端 runtime preferences 应答读取的本地 settings 根（S4-g）；None = 不注册监听。
    pub preferences_home: Option<PathBuf>,
    /// 握手身份 probe（S4-j）；None = 该装配不记录身份（仅测试/旧 Main 兼容）。
    pub identity_probe: Option<IdentityProbe>,
    /// Profile 重绑定时的凭据复制（S4-j）；缺 dataBaseDir 时 SSH 身份替换显式失败。
    pub credentials: Option<Arc<CredentialService>>,
}

impl ConnectionsOptions {
    pub fn new(
        client_version: impl Into<String>,
        resolve_transport: impl Fn(&crate::source_server::ConnectionProfile) -> TransportFuture
        + Send
        + Sync
        + 'static,
    ) -> Self {
        let client_version = client_version.into();
        Self {
            compatibility: CompatibilityManifest::local(client_version.clone()),
            client_version,
            client_id: DEFAULT_CLIENT_ID.to_owned(),
            initialize_timeout: INITIALIZE_TIMEOUT,
            resolve_transport: Arc::new(resolve_transport),
            install: None,
            recovery_delays: [
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8),
            ],
            startup_timeout: crate::source_server_recovery::STARTUP_RECONNECT_TIMEOUT,
            provisioning: None,
            preferences_home: None,
            identity_probe: None,
            credentials: None,
        }
    }

    /// 覆盖本地兼容清单（测试与 Main 显式注入 minCompatibleVersion 时使用）。
    pub fn with_compatibility(mut self, compatibility: CompatibilityManifest) -> Self {
        self.compatibility = compatibility;
        self
    }

    /// 显式连接时补齐远端缺失组件的配置（spec §11.3）。
    pub fn with_install(mut self, install: Option<InstallOptions>) -> Self {
        self.install = install;
        self
    }

    /// 覆盖自动恢复延迟（只用于测试；生产保持 2s → 4s → 8s）。
    pub fn with_recovery_delays(mut self, delays: [Duration; 3]) -> Self {
        self.recovery_delays = delays;
        self
    }

    /// 覆盖启动自动连接单次上限（只用于测试；生产保持 10s）。
    pub fn with_startup_timeout(mut self, timeout: Duration) -> Self {
        self.startup_timeout = timeout;
        self
    }

    /// 装配 Provider Provisioning 执行器（S4-g）；不设置时连接流程不发送信封。
    pub fn with_provisioning(
        mut self,
        provisioning: Option<Arc<SourceServerProviderProvisioning>>,
    ) -> Self {
        self.provisioning = provisioning;
        self
    }

    /// 装配远端 runtime preferences 的本地设置根（S4-g）。
    pub fn with_preferences_home(mut self, home: Option<PathBuf>) -> Self {
        self.preferences_home = home;
        self
    }

    /// 装配握手身份 probe（S4-j）。
    pub fn with_identity_probe(mut self, probe: Option<IdentityProbe>) -> Self {
        self.identity_probe = probe;
        self
    }

    /// 装配凭据服务，供身份替换时复制 SSH 凭据（S4-j）。
    pub fn with_credentials(mut self, credentials: Option<Arc<CredentialService>>) -> Self {
        self.credentials = credentials;
        self
    }
}

/// 连接意图：只有用户显式连接才安装缺失组件；自动恢复只连已部署的 server（spec §11.3/§11.5）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectIntent {
    Explicit,
    Recover,
    ConfirmIdentityChange,
}
