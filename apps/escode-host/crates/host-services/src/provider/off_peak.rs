//! Off-Peak 模型目录投影（TS `buildOffPeakModelSelectionView`）。

use anyhow::Result;
use serde_json::{Value, json};

use super::ProviderService;
use super::selection::project_provider;

impl ProviderService {
    /// Off-Peak 模型目录：只投影隐藏的固定 Off-Peak Provider（TS `buildOffPeakModelSelectionView`）。
    pub async fn off_peak_model_selection_view(&self) -> Result<Value> {
        let loaded = self.load().await?;
        let ids = [
            "account:zai-offpeak-idle-plan",
            "account:bigmodel-offpeak-idle-plan",
        ];
        let providers: Vec<Value> = loaded
            .registry
            .iter()
            .filter(|provider| {
                ids.contains(&provider.provider_id.as_str())
                    && provider.config.get("visibility").and_then(Value::as_str) == Some("hidden")
            })
            .map(project_provider)
            .collect();
        Ok(json!({"revision": loaded.revision, "providers": providers}))
    }
}
