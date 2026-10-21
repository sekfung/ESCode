//! `escode-agent` automation 本地管理方法（list/delete/enable/runs）。

use std::path::PathBuf;
use std::sync::Arc;

use escode_host_services::automation::AutomationService;
use serde_json::{Value, json};

use crate::automation_dispatch::{dispatch_run, track_outcome};
use crate::domains_task_state::StateContext;
use crate::domains_task_support::first_object;
use crate::session::{MethodHandler, handler};
use crate::wire::Wire;

#[allow(clippy::type_complexity)]
type AutomationRun =
    fn(
        Arc<AutomationService>,
        Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, String>> + Send>>;

pub(super) fn automation_methods(
    db_path: Option<PathBuf>,
    state: Option<StateContext>,
) -> Vec<(&'static str, MethodHandler)> {
    let automation = db_path.map(|path| Arc::new(AutomationService::from_db_path(path)));
    let state_for_dispatch = state;
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
        (
            "createAutomation",
            service(|s, p| Box::pin(async move { s.create(p).await })),
        ),
        (
            "updateAutomation",
            service(|s, p| {
                Box::pin(async move {
                    let id = p["automationId"].as_str().unwrap_or_default().to_owned();
                    s.update(id, p.clone(), workspace_key(&p))
                        .await?
                        .ok_or_else(|| "Automation 不存在或不属于当前工作区".to_owned())
                })
            }),
        ),
        (
            "restartAutomation",
            service(|s, p| {
                Box::pin(async move {
                    let id = p["automationId"].as_str().unwrap_or_default().to_owned();
                    s.restart(id, workspace_key(&p))
                        .await?
                        .ok_or_else(|| "Automation 不存在或不属于当前工作区".to_owned())
                })
            }),
        ),
        (
            "runAutomationNow",
            run_now_handler(automation.clone(), state_for_dispatch),
        ),
    ]
}

fn run_now_handler(
    automation: Option<Arc<AutomationService>>,
    state: Option<StateContext>,
) -> MethodHandler {
    match automation {
        Some(service) => handler(move |arg: Wire| {
            let service = Arc::clone(&service);
            let state = state.clone();
            let params = first_object(arg);
            Box::pin(async move {
                let id = params["automationId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                let Some((automation, run)) = service
                    .run_now(id.clone(), workspace_key(&params))
                    .await
                    .map_err(|message| ("Error".to_owned(), message))?
                else {
                    return Ok(Wire::Object(json!({"status": "duplicate"})));
                };
                let Some(state) = state else {
                    let _ = service
                        .fail_manual_run(
                            run["runId"].as_str().unwrap_or_default().to_owned(),
                            "Automation immediate dispatcher is unavailable.".to_owned(),
                        )
                        .await;
                    return Err((
                        "NotImplemented".to_owned(),
                        "Automation immediate dispatcher is unavailable.".to_owned(),
                    ));
                };
                let run_id = run["runId"].as_str().unwrap_or_default().to_owned();
                match dispatch_run(&state, &automation, &run).await {
                    Ok(dispatched) => {
                        let _ = service
                            .mark_manual_run_dispatched(
                                run_id.clone(),
                                Some(dispatched.task_id),
                                now(),
                            )
                            .await;
                        track_outcome(Arc::clone(&service), run_id, dispatched.terminal);
                        Ok(Wire::Object(json!({"status": "queued"})))
                    }
                    Err(error) => {
                        let _ = service.fail_manual_run(run_id, error.message.clone()).await;
                        Err((error.name, error.message))
                    }
                }
            })
        }),
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
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn workspace_key(params: &Value) -> Option<String> {
    let path = params["workspacePath"].as_str()?;
    let identity = params["workspaceIdentity"]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    Some(identity.unwrap_or(path).to_owned())
}
