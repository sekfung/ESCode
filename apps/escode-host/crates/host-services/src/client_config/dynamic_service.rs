//! `client-config.getDynamicWorkflowClientConfig`（TS `clientConfigService.ts`，spec §4）。

use futures_util::FutureExt;
use serde_json::Value;

use super::dynamic_workflow::{DynamicWorkflowConfig, normalize_mode, resolve};
use super::service::{CACHE_TTL_MS, ClientConfigService, ConfigError, fetch_payload, lock};

/// 用户选择：设置文件的 `dynamicWorkflowMode`（每次读；读不到按“跟随”处理）。
async fn read_user_mode(home: Option<&std::path::Path>) -> Option<Value> {
    let text = tokio::fs::read_to_string(crate::settings::settings_path(home?))
        .await
        .ok()?;
    serde_json::from_str::<Value>(&text)
        .ok()?
        .get("dynamicWorkflowMode")
        .cloned()
}

impl ClientConfigService {
    /// TS `getDynamicWorkflowClientConfig`：永不抛错（请求失败 fail-closed 为“未下发”）。
    pub async fn get_dynamic_workflow(&self, force_refresh: bool) -> DynamicWorkflowConfig {
        let dynamic = &self.inner.dynamic;
        let user_mode = read_user_mode(dynamic.settings_home.as_deref()).await;
        let env_override = dynamic.env_override.as_ref();
        // 覆盖合法即短路，不发请求。
        if normalize_mode(env_override).is_some() {
            return resolve(None, env_override, user_mode.as_ref());
        }
        match self.raw_envelope(force_refresh).await {
            Ok(payload) => resolve(
                payload.pointer("/data/configs/dynamicWorkflow"),
                env_override,
                user_mode.as_ref(),
            ),
            Err(error) => {
                escode_host_control::logging::warn(
                    "client-config",
                    format!(
                        "动态工作流灰度配置读取失败，按关闭处理: {}",
                        error.message()
                    ),
                );
                resolve(None, env_override, user_mode.as_ref())
            }
        }
    }

    /// TS `getRawConfigsEnvelope`：同端点同 TTL，在途请求合并（`forceRefresh` 也加入在途请求）。
    pub(super) async fn raw_envelope(&self, force_refresh: bool) -> Result<Value, ConfigError> {
        let pending = {
            let mut raw = lock(&self.inner.raw);
            if !force_refresh
                && let Some(payload) = &raw.payload
                && raw.expires_at > (self.inner.clock)()
            {
                return Ok(payload.clone());
            }
            if let Some(pending) = &raw.pending {
                pending.clone()
            } else {
                let this = self.clone();
                let pending = async move {
                    let result = async {
                        let url = this.request_url().await?;
                        fetch_payload(&this.inner, &url).await
                    }
                    .await;
                    let mut raw = lock(&this.inner.raw);
                    if let Ok(payload) = &result {
                        raw.payload = Some(payload.clone());
                        raw.expires_at = (this.inner.clock)() + CACHE_TTL_MS;
                    }
                    raw.pending = None;
                    result
                }
                .boxed()
                .shared();
                raw.pending = Some(pending.clone());
                pending
            }
        };
        pending.await
    }
}
