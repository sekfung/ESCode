//! `escode-agent` 旧面的补充方法（docs/specs/rust-u2-misc-channels.md §2–§3）。
//!
//! 这些成员不在 V4 连接方法表里，但 Renderer 仍按 `IESCodeAgentService` 直接调用：
//! 已保存工作流、LLM Wiki、Skill catalog、Session debug，以及插件旧面。
//! 事实源都在 CLI runtime；这里只做参数整形、路由与插件方法表复用。

use std::sync::Arc;

use serde_json::{Value, json};

use crate::domains_task_support::first_object;
use crate::forward::{self, Extract, RuntimePolicy, Shaped};
use crate::session::{EventHandler, MethodHandler, handler_rpc};
use crate::wire::Wire;

use super::base::Base;

/// 按 TS 字面量整形请求体后转发；`with_workspace` 决定是否补 `workspace` 引用。
fn shaped<F>(
    base: &Arc<Base>,
    method: &'static str,
    with_workspace: bool,
    build: F,
) -> MethodHandler
where
    F: Fn(&Value) -> Value + Send + Sync + 'static,
{
    let base = Arc::clone(base);
    handler_rpc(move |arg: Wire| {
        let base = Arc::clone(&base);
        let params = first_object(arg);
        let body = build(&params);
        Box::pin(async move {
            forward::runtime_call_shaped(
                &base.instances,
                &base.options,
                params,
                Shaped {
                    method,
                    body,
                    with_workspace,
                    trace: None,
                },
                Extract::Result,
            )
            .await
        })
    })
}

/// 已保存工作流：有 workspace 就直通；无 workspace 时按 TS 兜底打到插件管理实例。
fn saved_workflow<F>(base: &Arc<Base>, method: &'static str, build: F) -> MethodHandler
where
    F: Fn(&Value) -> Value + Send + Sync + 'static,
{
    let base = Arc::clone(base);
    handler_rpc(move |arg: Wire| {
        let base = Arc::clone(&base);
        let params = first_object(arg);
        let body = build(&params);
        Box::pin(async move {
            if params
                .get("workspacePath")
                .and_then(Value::as_str)
                .is_some()
            {
                return forward::runtime_call_shaped(
                    &base.instances,
                    &base.options,
                    params,
                    Shaped {
                        method,
                        body,
                        with_workspace: true,
                        trace: None,
                    },
                    Extract::Result,
                )
                .await;
            }
            let (_, workspace) = base.plugin_management.management_workspace()?;
            let instance = base.plugin_management.management_instance().await?;
            let mut body = body;
            if let Value::Object(record) = &mut body {
                record.insert("workspace".to_owned(), workspace);
            }
            forward::request(&instance, method, body, None)
                .await
                .map(Wire::Object)
        })
    })
}

fn optional(params: &Value, key: &str) -> Option<Value> {
    params.get(key).filter(|value| !value.is_null()).cloned()
}

fn with_optional(mut body: Value, params: &Value, keys: &[&str]) -> Value {
    if let Value::Object(record) = &mut body {
        for key in keys {
            if let Some(value) = optional(params, key) {
                record.insert((*key).to_owned(), value);
            }
        }
    }
    body
}

fn wiki_status(base: &Arc<Base>) -> MethodHandler {
    let base = Arc::clone(base);
    handler_rpc(move |arg: Wire| {
        let base = Arc::clone(&base);
        let params = first_object(arg);
        Box::pin(async move {
            let result = forward::runtime_call_shaped(
                &base.instances,
                &base.options,
                params,
                Shaped {
                    method: "v4/wiki/status",
                    body: json!({}),
                    with_workspace: false,
                    trace: None,
                },
                Extract::Result,
            )
            .await;
            match result {
                Err(error) if error.extra.get("code") == Some(&Value::from(-32601)) => {
                    Ok(Wire::Object(json!({"supported": false, "enabled": false})))
                }
                other => other,
            }
        })
    })
}

pub(super) fn legacy_methods(base: &Arc<Base>) -> Vec<(&'static str, MethodHandler)> {
    let mut methods = vec![
        (
            "listSavedWorkflows",
            saved_workflow(base, "workflows/list", |params| {
                with_optional(json!({}), params, &["scope"])
            }),
        ),
        (
            "getSavedWorkflow",
            saved_workflow(base, "workflows/get", |params| {
                with_optional(json!({"name": params["name"]}), params, &["scope"])
            }),
        ),
        (
            "updateSavedWorkflowMeta",
            saved_workflow(base, "workflows/updateMeta", |params| {
                with_optional(
                    json!({"name": params["name"], "meta": params["meta"]}),
                    params,
                    &["scope"],
                )
            }),
        ),
        (
            "deleteSavedWorkflow",
            saved_workflow(base, "workflows/delete", |params| {
                with_optional(json!({"name": params["name"]}), params, &["scope"])
            }),
        ),
        (
            "listSavedWorkflowRuns",
            saved_workflow(base, "workflows/runs", |params| {
                with_optional(
                    json!({"limit": params["limit"]}),
                    params,
                    &["name", "scope"],
                )
            }),
        ),
        (
            "moveSavedWorkflow",
            saved_workflow(
                base,
                "workflows/move",
                |params| json!({"name": params["name"]}),
            ),
        ),
        (
            "saveSavedWorkflowFromRun",
            saved_workflow(base, "workflows/save", |params| {
                with_optional(
                    json!({
                        "runId": params["runId"],
                        "name": params["name"],
                        "meta": params["meta"],
                    }),
                    params,
                    &["scope", "overwrite"],
                )
            }),
        ),
        (
            "findSavedWorkflowForRun",
            saved_workflow(base, "workflows/forRun", |params| {
                with_optional(json!({"runId": params["runId"]}), params, &["candidates"])
            }),
        ),
        ("getWikiStatus", wiki_status(base)),
        (
            "getWikiGraph",
            shaped(base, "v4/wiki/graph", false, |params| {
                optional(params, "query").unwrap_or_else(|| json!({}))
            }),
        ),
        (
            "getWikiEntry",
            shaped(
                base,
                "v4/wiki/entry",
                false,
                |params| json!({"id": params["id"]}),
            ),
        ),
        (
            "manageWiki",
            shaped(base, "v4/wiki/manage", false, |params| {
                params["request"].clone()
            }),
        ),
        (
            "getSkillReferenceCatalog",
            shaped(base, "skills/referenceCatalog", true, |params| {
                with_optional(json!({}), params, &["sessionId"])
            }),
        ),
    ];
    let prefs_base = Arc::clone(base);
    methods.push((
        "syncAppRuntimePreferences",
        handler_rpc(move |arg: Wire| {
            let base = Arc::clone(&prefs_base);
            let mut preferences = first_object(arg);
            preferences["modelIoFullRetentionEnabled"] =
                Value::Bool(preferences["modelIoFullRetentionEnabled"].as_bool() == Some(true));
            Box::pin(async move {
                base.set_preferences(preferences);
                base.sync_runtime_state().await.map(|_| Wire::Undefined)
            })
        }),
    ));
    let workflow_base = Arc::clone(base);
    methods.push((
        "syncDynamicWorkflowUserMode",
        handler_rpc(move |arg: Wire| {
            let base = Arc::clone(&workflow_base);
            let params = first_object(arg);
            let mode = params["mode"].as_str().unwrap_or("disabled").to_owned();
            Box::pin(async move {
                base.set_dynamic_workflow_mode(Some(Value::from(mode)));
                base.sync_runtime_state().await.map(|_| Wire::Undefined)
            })
        }),
    ));
    // `session/debug` 是观察面：只复用现有实例，不因打开调试面板拉起运行时。
    let debug_base = Arc::clone(base);
    methods.push((
        "readSessionDebug",
        handler_rpc(move |arg: Wire| {
            let base = Arc::clone(&debug_base);
            let params = first_object(arg);
            Box::pin(async move {
                forward::runtime_call_shaped_with_policy(
                    &base.instances,
                    &base.options,
                    params.clone(),
                    Shaped {
                        method: "session/debug",
                        body: json!({"sessionId": params["sessionId"]}),
                        with_workspace: false,
                        trace: None,
                    },
                    Extract::Result,
                    RuntimePolicy::ExistingOnly,
                )
                .await
            })
        }),
    ));
    // 插件旧面与 `plugin-management` 共用同一管理面实例/进度 Hub。
    let (plugin_methods, _) = base.plugin_management.aliases();
    methods.extend(plugin_methods);
    methods
}

pub(super) fn legacy_events(base: &Arc<Base>) -> Vec<(&'static str, EventHandler)> {
    let (_, mut events) = base.plugin_management.aliases();
    let observations = Arc::clone(&base.cua_observations);
    events.push((
        "onDynamicCuaPermissionObservation",
        Arc::new(move |arg: Wire| {
            let params = first_object(arg);
            let target = params["workspaceIdentity"]
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .or_else(|| params["workspacePath"].as_str())
                .map(str::to_owned);
            Ok(observations.event(target))
        }),
    ));
    events
}
