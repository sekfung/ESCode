//! `git.generateCommitMessage`（docs/specs/rust-u2-git.md §7；TS `gitService.generateCommitMessage` +
//! `GitCommitMessageGenerator`）。Git 侧输入与 prompt/校验在 `host-services`，这里只做模型编排：
//! 当前模型（Host 的 ModelSelection 视图 `preferredSelection`）→ 运行时 `workspace/generateText`。

use std::sync::Arc;
use std::time::Duration;

use escode_host_runtime::Instances;
use escode_host_services::git::GitService;
use escode_host_services::git::commit_message_prompt::{
    Failure, build_prompt, finalize_model_text,
};
use escode_host_services::provider::ProviderService;
use serde_json::{Value, json};

use crate::forward::{self, RuntimePolicy};
use crate::session::{MethodHandler, RpcError, handler_rpc};
use crate::wire::Wire;

/// TS `COMMIT_MESSAGE_QUERY_SOURCE`。
const QUERY_SOURCE: &str = "git_commit_message";
/// TS 不传 timeoutMs，协议客户端默认 3 分钟。
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
    git: Arc<GitService>,
    provider: Option<Arc<ProviderService>>,
    instances: Instances,
    options: escode_host_surface::OptionsFactory,
) -> MethodHandler {
    handler_rpc(move |arg: Wire| {
        let (git, provider, instances, options) = (
            Arc::clone(&git),
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
            let workspace_path = params["workspacePath"].as_str().unwrap_or_default();
            let include_unstaged = params["includeUnstaged"].as_bool().unwrap_or(true);
            let session_paths: Vec<String> = params["currentSessionFilePaths"]
                .as_array()
                .map(|list| {
                    list.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            let inputs = git
                .commit_message_inputs(workspace_path, include_unstaged, &session_paths)
                .await
                .map_err(|error| RpcError::new("Error", format!("{error:#}")))?;

            // 当前模型 = Host 的 ModelSelection 视图首选项（TS `modelSelection.getView().preferredSelection`）。
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
                inputs.branch_name.as_deref(),
                locale,
                Some(&runtime_locale),
                &inputs.files,
                &inputs.diffs,
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
