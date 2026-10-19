//! `client-config.getOffPeakClientConfig`（TS `clientConfigService.ts:184-197`）。

use serde_json::{Value, json};

use super::service::{ClientConfigService, ConfigError};

impl ClientConfigService {
    /// Off-Peak 入口配置：远端 `enable_offpeak_task` + 固定隐藏 Provider 的模型目录。
    pub async fn get_off_peak_client_config(
        &self,
        force_refresh: bool,
        model_selection_view: Value,
    ) -> Result<Value, ConfigError> {
        let payload = if self.inner.off_peak_mock {
            None
        } else {
            Some(self.raw_envelope(force_refresh).await?)
        };
        Ok(resolve(
            payload.as_ref(),
            self.inner.off_peak_mock,
            self.inner.off_peak_mock_no_plan,
            model_selection_view,
        ))
    }
}

pub(super) fn resolve(
    payload: Option<&Value>,
    mock: bool,
    mock_no_plan: bool,
    model_selection_view: Value,
) -> Value {
    let has_models = model_selection_view["providers"]
        .as_array()
        .is_some_and(|providers| {
            providers.iter().any(|provider| {
                provider["models"]
                    .as_array()
                    .is_some_and(|models| !models.is_empty())
            })
        });
    if mock {
        return json!({
            "enabled": has_models,
            "modelSelectionView": model_selection_view,
            "codingPlanActive": !mock_no_plan,
        });
    }
    json!({
        "enabled": payload
            .and_then(|payload| payload.pointer("/data/configs/offPeak/enable_offpeak_task"))
            .and_then(Value::as_bool) == Some(true)
            && has_models,
        "modelSelectionView": model_selection_view,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(models: usize) -> Value {
        json!({"revision": 1, "providers": [{"providerId": "account:zai-offpeak-idle-plan", "models": (0..models).map(|i| json!({"modelId": format!("m{i}")})).collect::<Vec<_>>()}]})
    }

    #[test]
    fn mock_short_circuits_and_uses_model_presence() {
        assert_eq!(resolve(None, true, false, view(1))["enabled"], true);
        assert_eq!(
            resolve(None, true, false, view(1))["codingPlanActive"],
            true
        );
        assert_eq!(
            resolve(None, true, true, view(1))["codingPlanActive"],
            false
        );
        assert_eq!(resolve(None, true, false, view(0))["enabled"], false);
    }

    #[test]
    fn remote_flag_and_models_are_both_required() {
        let enabled = json!({"data": {"configs": {"offPeak": {"enable_offpeak_task": true}}}});
        assert_eq!(
            resolve(Some(&enabled), false, false, view(1))["enabled"],
            true
        );
        assert_eq!(
            resolve(Some(&enabled), false, false, view(0))["enabled"],
            false
        );
        assert_eq!(
            resolve(Some(&json!({})), false, false, view(1))["enabled"],
            false
        );
    }
}
