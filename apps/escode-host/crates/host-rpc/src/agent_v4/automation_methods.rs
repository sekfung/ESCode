//! `escode-agent` automation 本地管理方法（list/delete/enable/runs）。

use std::path::PathBuf;
use std::sync::Arc;

use escode_host_services::automation::AutomationService;
use serde_json::{Value, json};

use crate::domains_task_support::first_object;
use crate::session::{MethodHandler, handler};
use crate::wire::Wire;

#[allow(clippy::type_complexity)]
type AutomationRun =
    fn(
        Arc<AutomationService>,
        Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, String>> + Send>>;

pub(super) fn automation_methods(db_path: Option<PathBuf>) -> Vec<(&'static str, MethodHandler)> {
    let automation = db_path.map(|path| Arc::new(AutomationService::from_db_path(path)));
    let service = |run: AutomationRun| -> MethodHandler {
        match &automation {
            Some(service) => {
                let service = Arc::clone(service);
                handler(move |arg: Wire| {
                    let (service, params) = (Arc::clone(&service), first_object(arg));
                    Box::pin(async move {
                        run(service, params)
                            .await
                            .map(Wire::Object)
                            .map_err(|message| ("Error".to_owned(), message))
                    })
                })
            }
            None => handler(|_arg: Wire| {
                Box::pin(async move {
                    Err((
                        "NotImplemented".to_owned(),
                        "automation needs dataBaseDir: Main must pass dataBaseDir in rpcListen."
                            .to_owned(),
                    ))
                })
            }),
        }
    };
    vec![
        (
            "listAutomations",
            service(|s, p| Box::pin(async move { s.list(workspace_key(&p)).await })),
        ),
        (
            "listAllAutomations",
            service(|s, _p| Box::pin(async move { s.list(None).await })),
        ),
        (
            "deleteAutomation",
            service(|s, p| {
                Box::pin(async move {
                    let id = p["automationId"].as_str().unwrap_or_default().to_owned();
                    s.delete(id, workspace_key(&p)).await.map(|_| json!({}))
                })
            }),
        ),
        (
            "setAutomationEnabled",
            service(|s, p| {
                Box::pin(async move {
                    let id = p["automationId"].as_str().unwrap_or_default().to_owned();
                    let enabled = p["enabled"].as_bool().unwrap_or(false);
                    s.set_enabled(id, enabled, workspace_key(&p))
                        .await
                        .map(|_| json!({}))
                })
            }),
        ),
        (
            "listAutomationRuns",
            service(|s, p| {
                Box::pin(async move {
                    let id = p["automationId"].as_str().unwrap_or_default().to_owned();
                    s.list_runs(id, workspace_key(&p)).await
                })
            }),
        ),
        (
            "deleteAutomationRun",
            service(|s, p| {
                Box::pin(async move {
                    let id = p["runId"].as_str().unwrap_or_default().to_owned();
                    s.delete_run(id, workspace_key(&p)).await.map(|_| json!({}))
                })
            }),
        ),
    ]
}

fn workspace_key(params: &Value) -> Option<String> {
    let path = params["workspacePath"].as_str()?;
    let identity = params["workspaceIdentity"]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    Some(identity.unwrap_or(path).to_owned())
}
