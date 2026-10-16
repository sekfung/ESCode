//! `provider-settings.testModelConnectivity`（TS `providerFacadeServices.ts` + `providerSettingsConnectivity.ts`）。
//!
//! 先等同 Provider 的写入收口（TS `waitForProviderOperations`），再用公共资格判定可测性，通过后把
//! 正式 ModelSelection 交给该 workspace 的 runtime（`provider/testModelConnectivity`）。
//! 登记差异：TS 在调用前会同步账号 Provider 配置（`ensureAccountProviderConfigSynced`），Rust 暂无账号源。

use std::sync::Arc;
use std::time::Duration;

use escode_host_runtime::Instances;
use escode_host_services::provider::ProviderService;
use serde_json::{Value, json};

use crate::forward::{self, RuntimePolicy};
use crate::session::{MethodHandler, RpcError, handler_rpc};
use crate::wire::Wire;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

fn failure(code: Option<&str>, message: &str) -> Value {
    let mut error = json!({"message": message});
    if let Some(code) = code {
        error["code"] = Value::from(code);
    }
    json!({"success": false, "error": error})
}

/// TS 的资格判定链：provider 缺失/禁用 → model 缺失/禁用/有问题 → provider 不可执行 → model 不可执行。
pub(crate) fn unavailable(view: &Value, provider_id: &str, model_id: &str) -> Option<Value> {
    let provider = view["providers"]
        .as_array()
        .and_then(|list| list.iter().find(|p| p["providerId"] == provider_id));
    let model = provider
        .and_then(|p| p["models"].as_array())
        .and_then(|list| list.iter().find(|m| m["modelId"] == model_id));
    let code = match (provider, model) {
        (None, _) => "provider-unavailable",
        (Some(p), _) if p["enabled"] != true => "provider-unavailable",
        (_, None) => "model-unavailable",
        (_, Some(m))
            if m["enabled"] != true || m["issues"].as_array().is_some_and(|i| !i.is_empty()) =>
        {
            "model-unavailable"
        }
        (Some(p), _) if p["executable"] != true => "provider-unavailable",
        (_, Some(m)) if m["executable"] != true => "model-unavailable",
        _ => return None,
    };
    let message = if code == "provider-unavailable" {
        "This provider is currently unavailable for connectivity testing."
    } else {
        "This model is currently unavailable for connectivity testing."
    };
    Some(failure(Some(code), message))
}

pub(crate) fn connectivity_method(
    service: Option<Arc<ProviderService>>,
    instances: Instances,
    options: escode_host_surface::OptionsFactory,
) -> MethodHandler {
    handler_rpc(move |arg: Wire| {
        let (service, instances, options) =
            (service.clone(), instances.clone(), Arc::clone(&options));
        Box::pin(async move {
            let Some(service) = service else {
                return Err(RpcError::new(
                    "InvalidRequest",
                    "Host has no provider config files.",
                ));
            };
            let input = arg
                .into_args()
                .into_iter()
                .next()
                .and_then(Wire::into_object)
                .unwrap_or(Value::Null);
            let provider_id = input["providerId"].as_str().unwrap_or_default().to_owned();
            let model_id = input["modelId"].as_str().unwrap_or_default().to_owned();
            service.wait_for_writes().await;
            let view = service
                .provider_settings_view()
                .await
                .map_err(|error| RpcError::new("Error", format!("{error:#}")))?;
            if let Some(result) = unavailable(&view, &provider_id, &model_id) {
                return Ok(Wire::Object(result));
            }
            let target = forward::runtime_target(input.clone())
                .map_err(<RpcError as From<(String, String)>>::from)?;
            let body = json!({
                "workspace": target.request["workspace"],
                "selection": {"providerId": provider_id, "modelId": model_id},
            });
            let outcome = async {
                let instance = forward::runtime_instance(
                    &instances,
                    &options,
                    &target,
                    RuntimePolicy::StartIfNeeded,
                )
                .await?;
                forward::request_with_timeout(
                    &instance,
                    "provider/testModelConnectivity",
                    body,
                    None,
                    REQUEST_TIMEOUT,
                )
                .await
            }
            .await;
            Ok(Wire::Object(match outcome {
                Ok(_) => json!({"success": true}),
                Err(error) => failure(None, &error.message),
            }))
        })
    })
}

#[cfg(test)]
#[path = "domains_provider_connectivity_tests.rs"]
mod tests;
