//! `SourceServerConnections` 的 Provider Provisioning 辅助（S4-g）。
//!
//! 连接事实仍由 [`SourceServerConnections`] 唯一拥有；这里只负责首连屏障与在线触发扇出。

use std::sync::Arc;

use crate::source_server::ConnectionProfile;
use crate::source_server_client::RemoteChannelClient;
use crate::source_server_provider_provisioning::{
    ProviderProvisioningFailure, ProviderProvisioningPeer, ProvisioningStatus, ProvisioningTrigger,
    RemoteProvisioningPeer,
};

use super::{ConnectIntent, SourceServerConnections};

impl SourceServerConnections {
    /// 发布 online 前的屏障：只对「显式连接且从未在线」失败；恢复/重连只 warning。
    pub(crate) async fn provision_before_online(
        &self,
        profile: &ConnectionProfile,
        profile_id: &str,
        attempt: u64,
        client: &Arc<RemoteChannelClient>,
        intent: ConnectIntent,
    ) -> anyhow::Result<()> {
        let Some(provisioning) = self.options().provisioning.clone() else {
            return Ok(());
        };
        let peer: Arc<dyn ProviderProvisioningPeer> =
            Arc::new(RemoteProvisioningPeer::new(Arc::clone(client)));
        let outcome = provisioning
            .sync(
                profile_id,
                attempt,
                peer,
                ProvisioningTrigger::EnvironmentOnline,
            )
            .await;
        if outcome.allows_online() {
            return Ok(());
        }
        // 连接在 provisioning 期间被收口：不覆盖新状态。
        if self.current_attempt(profile_id) != attempt {
            return Ok(());
        }
        if self.requires_provisioning_barrier(profile, intent) {
            let code = outcome.failure_code();
            let failure = ProviderProvisioningFailure {
                code,
                message: format!("{:#}", outcome.failure()),
                retryable: !matches!(outcome.status, ProvisioningStatus::Unsupported),
            };
            return Err(self
                .state()
                .fail(profile, anyhow::Error::new(failure), Some(code)));
        }
        provisioning.log_warning(profile_id, ProvisioningTrigger::EnvironmentOnline, &outcome);
        Ok(())
    }

    /// 后续 Provider 事实变化：对全部在线连接推进一次 generation（同 profile 合并尾随同步）。
    pub fn notify_provider_provisioning(&self, trigger: ProvisioningTrigger) {
        let Some(provisioning) = self.options().provisioning.clone() else {
            return;
        };
        let active: Vec<(String, u64, Arc<RemoteChannelClient>)> = {
            let slots = self.lock();
            slots
                .iter()
                .filter_map(|(profile_id, slot)| {
                    slot.connection.as_ref().map(|connection| {
                        (
                            profile_id.clone(),
                            slot.attempt,
                            Arc::clone(&connection.client),
                        )
                    })
                })
                .collect()
        };
        for (profile_id, attempt, client) in active {
            let peer: Arc<dyn ProviderProvisioningPeer> =
                Arc::new(RemoteProvisioningPeer::new(client));
            provisioning.request(&profile_id, attempt, peer, trigger);
        }
    }

    /// 首连屏障只作用于「显式连接且该 Source 从未记录 lastOnlineAt」；恢复/重连只 warning。
    fn requires_provisioning_barrier(
        &self,
        profile: &ConnectionProfile,
        intent: ConnectIntent,
    ) -> bool {
        if !matches!(
            intent,
            ConnectIntent::Explicit | ConnectIntent::ConfirmIdentityChange
        ) {
            return false;
        }
        profile
            .source_server_id
            .as_deref()
            .and_then(|source_server_id| self.registry().source_server(source_server_id))
            .and_then(|record| record.last_online_at)
            .is_none()
    }
}
