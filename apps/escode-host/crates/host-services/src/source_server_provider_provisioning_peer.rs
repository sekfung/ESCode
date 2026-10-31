//! Provider Provisioning 的远端 peer（S4-g）：把 Envelope 交给远端 target。

use std::sync::Arc;

use anyhow::{Result, anyhow};
use escode_host_core::rpc_serialization::WireValue;
use serde_json::Value;

use crate::source_server_client::RemoteChannelClient;

pub const PROVIDER_PROVISIONING_TARGET_CHANNEL: &str = "provider-provisioning-target";

#[async_trait::async_trait]
pub trait ProviderProvisioningPeer: Send + Sync {
    fn supports_target(&self) -> bool;
    async fn apply(&self, envelope: Value) -> Result<Value>;
}

/// 活动远端连接的临时 peer；连接被收口后调用会显式失败。
pub struct RemoteProvisioningPeer {
    client: Arc<RemoteChannelClient>,
}

impl RemoteProvisioningPeer {
    pub fn new(client: Arc<RemoteChannelClient>) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl ProviderProvisioningPeer for RemoteProvisioningPeer {
    fn supports_target(&self) -> bool {
        self.client
            .has_channel(PROVIDER_PROVISIONING_TARGET_CHANNEL)
    }

    async fn apply(&self, envelope: Value) -> Result<Value> {
        let result = self
            .client
            .call(
                PROVIDER_PROVISIONING_TARGET_CHANNEL,
                "apply",
                // TS `ProxyChannel.call(ctx, command, args[])` 用 `apply(handler, args)` 展开实参，
                // 信封必须作为单元素数组下发；裸对象会让远端 TypeError 后误报 target-call-failed。
                Some(WireValue::Array(vec![WireValue::from_json(&envelope)])),
            )
            .await?;
        result
            .to_json()
            .ok_or_else(|| anyhow!("provider-provisioning-target.apply returned a non-JSON result"))
    }
}
