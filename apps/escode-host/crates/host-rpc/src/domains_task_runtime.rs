//! `escode-task` 里直接作用于运行时进程的方法。
//!
//! `restartWorkspaceProcess`（TS `escodeTaskServiceAdapter.ts:3271` → `escodeAgentService.disposeWorkspace`）：
//! Renderer 在「重新加载工作区会话」时调用，使 runtime 失效；其后的 `prepareWorkspace` 等调用会懒重启同一身份的实例。
//! 注意它**不是**会话/任务的 teardown：任务目标注册表、V4 订阅与 workspace 事件监听保持不变（TS 同注释）。

use escode_host_control::logging;
use escode_host_runtime::Instances;
use serde_json::Value;

use crate::domains_task_support::{RuntimeAccess, first_object};
use crate::session::{MethodHandler, handler};
use crate::wire::Wire;

/// 身份键：`workspaceIdentity?.trim() || workspacePath`。
fn identity_of(params: &Value) -> String {
    params["workspaceIdentity"]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| params["workspacePath"].as_str())
        .unwrap_or_default()
        .to_owned()
}

pub(super) fn restart_workspace_process(runtime: Option<RuntimeAccess>) -> MethodHandler {
    let instances: Option<Instances> = runtime.map(|runtime| runtime.instances);
    handler(move |arg: Wire| {
        let params = first_object(arg);
        let instances = instances.clone();
        Box::pin(async move {
            let Some(instances) = instances else {
                return Err((
                    "InvalidRequest".to_owned(),
                    "Host has no runtime access.".to_owned(),
                ));
            };
            let identity = identity_of(&params);
            if identity.is_empty() {
                return Err((
                    "InvalidRequest".to_owned(),
                    "workspacePath is required.".to_owned(),
                ));
            }
            // 收口失败（残余进程）只记录：TS `disposeWorkspace` 不向调用方抛进程清理细节。
            match instances.close(&identity).await {
                Ok(residual) if residual.is_empty() => {}
                Ok(residual) => logging::warn(
                    "task-runtime",
                    format!(
                        "restartWorkspaceProcess left residual tasks identity={identity}: {residual:?}"
                    ),
                ),
                Err(error) => logging::warn(
                    "task-runtime",
                    format!("restartWorkspaceProcess close failed identity={identity}: {error:#}"),
                ),
            }
            Ok(Wire::Undefined)
        })
    })
}

/// 不依赖任何状态的 `escode-task` 方法（TS 适配器里就是常量应答）：
/// `getWorkspaceProviderConfigFile` / `getTaskSessionFilePath` / `checkCodexConnectivity`。
pub(super) fn constant_methods() -> Vec<(&'static str, MethodHandler)> {
    vec![
        (
            "getWorkspaceProviderConfigFile",
            handler(|arg: Wire| {
                let params = first_object(arg);
                Box::pin(async move {
                    Ok(Wire::Object(serde_json::json!({
                        "provider": crate::domains_task_support::GLM_PROVIDER,
                        "path": params["workspacePath"],
                        "exists": false,
                    })))
                })
            }),
        ),
        (
            "getTaskSessionFilePath",
            handler(|arg: Wire| {
                let params = first_object(arg);
                Box::pin(async move {
                    let workspace = params["workspacePath"].as_str().unwrap_or_default();
                    let task = params["taskId"].as_str().unwrap_or_default();
                    Ok(Wire::Object(serde_json::json!({
                        "path": format!("{workspace}/{task}.escode-session"),
                        "exists": false,
                    })))
                })
            }),
        ),
        (
            "checkCodexConnectivity",
            handler(|_arg: Wire| {
                Box::pin(async move {
                    Ok(Wire::Object(
                        serde_json::json!({"target": "escode-agent", "reachable": true}),
                    ))
                })
            }),
        ),
    ]
}
