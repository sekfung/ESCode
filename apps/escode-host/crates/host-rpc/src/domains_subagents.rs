//! `subagents` channel（TS `ServiceChannels.Subagents` = `"subagents"`；docs/specs/rust-u2-subagents.md）。
//!
//! 已接 5/8：`list` + `getPrimaryUserAgentsDirectory` + `setEnabled` + 两个模型覆盖。
//! 其余 3 个（`createAgent` / `updateAgent` / `deleteAgent`）依赖 markdown 序列化与改名时的
//! 禁用态迁移，由 `respond` 回显式缺口并列出已实现集合。

use std::path::PathBuf;
use std::sync::Arc;

use escode_host_services::subagents::SubagentsService;
use serde_json::{Value, json};

use crate::domains_task_support::first_object;
use crate::session::{Channel, MethodHandler, handler};
use crate::wire::Wire;

fn failed(error: String) -> (String, String) {
    ("Error".to_owned(), error)
}

/// 四个方法共用的样板：解包参数 → 调服务 → 固定应答形状。
fn method<F>(service: Arc<SubagentsService>, run: F) -> MethodHandler
where
    F: Fn(
            Arc<SubagentsService>,
            Value,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>>
        + Send
        + Sync
        + 'static,
{
    handler(move |arg: Wire| {
        let (service, params) = (Arc::clone(&service), first_object(arg));
        let future = run(service, params);
        Box::pin(async move {
            future.await.map_err(failed)?;
            Ok(Wire::Undefined)
        })
    })
}

pub(crate) fn subagents_channel(home: PathBuf) -> Channel {
    let service = Arc::new(SubagentsService::new(home));

    let primary = {
        let service = Arc::clone(&service);
        handler(move |_arg: Wire| {
            let service = Arc::clone(&service);
            Box::pin(async move {
                let path = service
                    .primary_user_agents_directory()
                    .await
                    .map_err(failed)?;
                Ok::<Wire, (String, String)>(Wire::Object(json!({ "path": path })))
            })
        })
    };
    let set_enabled = method(Arc::clone(&service), |service, params| {
        Box::pin(async move {
            let agent_id = params["agentId"].as_str().unwrap_or_default().to_owned();
            let enabled = params["enabled"].as_bool().unwrap_or(false);
            service.set_enabled(&agent_id, enabled).await
        })
    });
    let set_built_in = method(Arc::clone(&service), |service, params| {
        Box::pin(async move {
            let agent_name = params["agentName"].as_str().unwrap_or_default().to_owned();
            let selection = params.get("modelSelection").cloned();
            service
                .set_built_in_model_override(&agent_name, selection.as_ref())
                .await
        })
    });
    let set_plugin = method(Arc::clone(&service), |service, params| {
        Box::pin(async move {
            let agent_id = params["agentId"].as_str().unwrap_or_default().to_owned();
            let selection = params.get("modelSelection").cloned();
            service
                .set_plugin_agent_model_override(&agent_id, selection.as_ref())
                .await
        })
    });

    let list_method = {
        let service = Arc::clone(&service);
        handler(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                let workspace_path = params["workspacePath"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                let mode = params["mode"].as_str().map(str::to_owned);
                let answer = escode_host_services::subagents::list::list(
                    service.home(),
                    escode_host_services::subagents::list::ListParams {
                        workspace_path: &workspace_path,
                        mode: mode.as_deref(),
                    },
                    &crate::collation::compare_names,
                )
                .await
                .map_err(failed)?;
                Ok::<Wire, (String, String)>(Wire::Object(answer))
            })
        })
    };

    Channel::new(
        "subagents",
        vec![
            ("list", list_method),
            ("getPrimaryUserAgentsDirectory", primary),
            ("setEnabled", set_enabled),
            ("setBuiltInModelOverride", set_built_in),
            ("setPluginAgentModelOverride", set_plugin),
        ],
    )
}
