//! `svn.generateCommitMessage`（docs/svn/architecture.md「ISvnService」）。
//!
//! 输入（勾选文件、行数与 diff）由 svn 服务收集；prompt 与输出校验复用 Git 的共享实现，
//! 这里只做模型编排：当前模型 → 运行时 `workspace/generateText`。

use std::sync::Arc;
use std::time::Duration;

use escode_host_runtime::Instances;
use escode_host_services::git::commit_message_prompt::{
    Failure, build_prompt, finalize_model_text,
};
use escode_host_services::provider::ProviderService;
use escode_host_services::svn::SvnService;
use serde_json::{Value, json};

use crate::forward::{self, RuntimePolicy};
use crate::session::{MethodHandler, RpcError, handler_rpc};
use crate::wire::Wire;

/// TS `COMMIT_MESSAGE_QUERY_SOURCE`（SVN 复用 Git 生成器同一值）。
const QUERY_SOURCE: &str = "git_commit_message";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);

fn generation_error(message: &str, detail: Option<String>) -> RpcError {
    let error = RpcError::new("GitCommitMessageGenerationError", message);
    match detail {
        Some(detail) => error.with("detail", Value::String(detail)),
        None => error,
    }
}

fn failure(failure: Failure) -> RpcError {
    generation_error(failure.message, failure.detail)
}

pub(crate) fn generate_commit_message_method(
    service: Arc<SvnService>,
    provider: Option<Arc<ProviderService>>,
    instances: Instances,
    options: escode_host_surface::OptionsFactory,
) -> MethodHandler {
    handler_rpc(move |arg: Wire| {
        let (service, provider, instances, options) = (
            Arc::clone(&service),
            provider.clone(),
            instances.clone(),
            Arc::clone(&options),
        );
        Box::pin(async move {
            let params = arg
                .into_args()
                .into_iter()
                .next()
                .and_then(Wire::into_object)
                .unwrap_or(Value::Null);
            let (branch_name, files, diffs) = service
                .commit_message_inputs(&params)
                .await
                .map_err(|error| RpcError::new("Error", format!("{error:#}")))?;
            let selection = match &provider {
                Some(provider) => provider
                    .model_selection_view(None)
                    .await
                    .map_err(|error| RpcError::new("Error", format!("{error:#}")))?
                    .get("preferredSelection")
                    .cloned(),
                None => None,
            };
            let selection = selection
                .filter(|s| !s.is_null())
                .and_then(|s| {
                    let provider_id = s["providerId"].as_str()?.trim().to_owned();
                    let model_id = s["modelId"].as_str()?.trim().to_owned();
                    if provider_id.is_empty() || model_id.is_empty() {
                        return None;
                    }
                    let mut out = json!({"providerId": provider_id, "modelId": model_id});
                    if let Some(options) = s.get("options").filter(|o| o.is_object()) {
                        out["options"] = options.clone();
                    }
                    Some(out)
                })
                .ok_or_else(|| generation_error("未读取到当前模型。", None))?;

            let locale = params["locale"].as_str();
            let runtime_locale = crate::collation::runtime_locale();
            let prompt = build_prompt(
                branch_name.as_deref(),
                locale,
                Some(&runtime_locale),
                &files,
                &diffs,
                params.get("conversationContext").unwrap_or(&Value::Null),
            );
            let target = forward::runtime_target(params.clone())
                .map_err(<RpcError as From<(String, String)>>::from)?;
            let body = json!({
                "workspace": target.request["workspace"],
                "selection": selection,
                "prompt": prompt,
                "querySource": QUERY_SOURCE,
            });
            let text = async {
                let instance = forward::runtime_instance(
                    &instances,
                    &options,
                    &target,
                    RuntimePolicy::StartIfNeeded,
                )
                .await?;
                forward::request_with_timeout(
                    &instance,
                    "workspace/generateText",
                    body,
                    None,
                    REQUEST_TIMEOUT,
                )
                .await
            }
            .await
            .map_err(|error| generation_error("模型请求失败。", Some(error.message)))?;
            let message =
                finalize_model_text(text["text"].as_str().unwrap_or_default()).map_err(failure)?;
            Ok(Wire::Object(json!({
                "message": message,
                "providerId": selection["providerId"],
                "model": selection["modelId"],
            })))
        })
    })
}
