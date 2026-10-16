//! `skills` channel（docs/specs/rust-u2-skills.md）。

use std::path::Path;
use std::sync::Arc;

use escode_host_services::skills::{Compare, SkillsError, SkillsService};
use serde_json::Value;

use crate::channel::Channel;
use crate::session::{MethodHandler, RpcError, handler_rpc};
use crate::wire::Wire;

fn method<F, Fut>(service: &Arc<SkillsService>, run: F) -> MethodHandler
where
    F: Fn(Arc<SkillsService>, Value) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Wire, SkillsError>> + Send + 'static,
{
    let service = Arc::clone(service);
    let run = Arc::new(run);
    handler_rpc(move |arg: Wire| {
        let (service, run) = (Arc::clone(&service), Arc::clone(&run));
        Box::pin(async move {
            let params = arg
                .into_args()
                .into_iter()
                .next()
                .and_then(Wire::into_object)
                .unwrap_or(Value::Null);
            run(service, params)
                .await
                .map_err(|error| RpcError::new("Error", error.0))
        })
    })
}

fn text(params: &Value, key: &str) -> String {
    params[key].as_str().unwrap_or_default().to_owned()
}

pub(crate) fn skills_channel(home: &Path) -> Channel {
    let compare: Compare = Arc::new(crate::collation::compare_names);
    let desktop =
        escode_cli_core_api::env::var("ESCODE_PROCESS_LABEL").is_ok_and(|v| !v.is_empty());
    let service = Arc::new(SkillsService::new(home.to_path_buf(), compare, desktop));
    Channel::new(
        "skills",
        vec![
            (
                "list",
                method(&service, |s, p| async move {
                    Ok(Wire::Object(s.list(&text(&p, "workspacePath")).await))
                }),
            ),
            (
                "setEnabled",
                method(&service, |s, p| async move {
                    s.set_enabled(
                        &text(&p, "workspacePath"),
                        &text(&p, "skillId"),
                        p["enabled"].as_bool().unwrap_or(false),
                    )
                    .await
                    .map(|()| Wire::Undefined)
                }),
            ),
            (
                "buildPromptContext",
                method(&service, |s, p| async move {
                    let identity = p["workspaceIdentity"].as_str().map(str::to_owned);
                    s.build_prompt_context(
                        &text(&p, "workspacePath"),
                        identity.as_deref(),
                        &text(&p, "prompt"),
                    )
                    .await
                    .map(Wire::Object)
                }),
            ),
            (
                "copyToCommon",
                method(&service, |s, p| async move {
                    s.copy_to_common(&text(&p, "workspacePath"), &text(&p, "skillId"))
                        .await
                        .map(Wire::Object)
                }),
            ),
            (
                "removeFromCommon",
                method(&service, |s, p| async move {
                    s.remove_from_common(&text(&p, "workspacePath"), &text(&p, "skillId"))
                        .await
                        .map(|()| Wire::Undefined)
                }),
            ),
            (
                "deleteSkill",
                method(&service, |s, p| async move {
                    s.delete_skill(&text(&p, "workspacePath"), &text(&p, "skillId"))
                        .await
                        .map(|()| Wire::Undefined)
                }),
            ),
        ],
    )
}
