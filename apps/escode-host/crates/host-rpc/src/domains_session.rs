//! `escode-session` channel（从 `domains.rs` 拆出以守住单文件行数上限）。

use std::sync::Arc;

use serde_json::{Value, json};

use crate::domains_session_write::SessionWriteContext;
use crate::forward::{Extract, Shaped, runtime_call, runtime_call_shaped, runtime_call_with};
use crate::session::{Channel, handler};
use crate::wire::Wire;

/// `escode-session`（TS `ServiceChannels.ESCodeSession` = `"escode-session"`）：
/// 转发按 TS `escodeAgentService.ts` 的请求字面量整形（`session_forward_params`，评审 A11），
/// 带 Host 职责的方法（草稿/索引/thoughtLevel 重放/`withApiRetryRuntime`）见 `domains_session_write.rs`。
/// 未迁移：`readSessionEvents`（运行时无 `session/events`）与 workspace/provider 握手类方法（属各自域）。
pub(crate) fn session_channel(
    instances: escode_host_runtime::Instances,
    options: escode_host_surface::OptionsFactory,
    write: SessionWriteContext,
    provider_headers: Option<std::sync::Arc<crate::provider_headers::ProviderHeaders>>,
) -> Channel {
    use crate::domains_session_write as w;

    fn params_of(arg: Wire) -> Value {
        arg.into_args()
            .into_iter()
            .next()
            .and_then(Wire::into_object)
            .unwrap_or(Value::Null)
    }

    // 只做白名单整形与结果抽取的转发。
    let forward = |method: &'static str, with_workspace: bool, extract: Extract| {
        let instances = instances.clone();
        let options = Arc::clone(&options);
        crate::session::handler_rpc(move |arg: Wire| {
            let instances = instances.clone();
            let options = Arc::clone(&options);
            Box::pin(async move {
                let params = params_of(arg);
                let (runtime_method, body) =
                    escode_host_services::session_params::session_forward_params(method, &params)
                        .ok_or_else(|| {
                        crate::session::RpcError::new(
                            "Fatal",
                            format!("no runtime mapping for {method}"),
                        )
                    })?;
                let shaped = Shaped {
                    method: runtime_method,
                    body,
                    with_workspace,
                    trace: None,
                };
                runtime_call_shaped(&instances, &options, params, shaped, extract).await
            })
        })
    };

    // 写索引的方法需要 dataBaseDir：缺省时回显式缺口，不静默成功。
    let indexed = |build: fn(SessionWriteContext) -> crate::session::MethodHandler| {
        if write.read.is_some() {
            build(write.clone())
        } else {
            handler(|_arg: Wire| {
                Box::pin(async move {
                    Err((
                        "NotImplemented".to_owned(),
                        "escode-session writes need dataBaseDir: Main must pass dataBaseDir in rpcListen."
                            .to_owned(),
                    ))
                })
            })
        }
    };

    Channel::new(
        "escode-session",
        vec![
            ("createSession", indexed(w::create_session_handler)),
            ("resumeSession", indexed(w::resume_session_handler)),
            ("setModel", indexed(w::set_model_handler)),
            (
                "listSessions",
                forward("listSessions", true, Extract::Field("sessions")),
            ),
            (
                "readSession",
                w::api_retry_forward_handler(write.clone(), "readSession"),
            ),
            (
                "readSessionMessages",
                forward("readSessionMessages", false, Extract::Field("messages")),
            ),
            ("closeSession", w::close_session_handler(write.clone())),
            (
                "closeDeferredDraftSession",
                super::domains_session_drafts::close_deferred_draft_handler(write.clone()),
            ),
            (
                "promoteDeferredDraftSession",
                super::domains_session_drafts::promote_deferred_draft_handler(write.clone()),
            ),
            (
                "setThoughtLevel",
                w::api_retry_forward_handler(write.clone(), "setThoughtLevel"),
            ),
            (
                "setMode",
                w::api_retry_forward_handler(write.clone(), "setMode"),
            ),
            (
                "getWorkspaceRuntimeIdentity",
                handler({
                    let instances = instances.clone();
                    move |arg: Wire| {
                        let instances = instances.clone();
                        Box::pin(async move {
                            let target = crate::forward::runtime_target(params_of(arg))?;
                            let Some(instance) = instances.live(&target.identity) else {
                                return Err((
                                    "Error".to_owned(),
                                    "ESCode agent runtime identity is unavailable.".to_owned(),
                                ));
                            };
                            Ok(Wire::Object(json!({
                                "generation": instance.generation,
                                "identity": instance.identity,
                                "workspaceKey": instance.identity,
                            })))
                        })
                    }
                }),
            ),
            (
                "respondProviderRuntimeHeaders",
                match provider_headers {
                    Some(headers) => crate::session::handler(move |arg: Wire| {
                        let headers = std::sync::Arc::clone(&headers);
                        Box::pin(async move {
                            let params = params_of(arg);
                            let request_id = params["requestId"].as_str().unwrap_or_default();
                            headers
                                .respond(request_id, &params["response"])
                                .map(|_| Wire::Undefined)
                                .map_err(|message| ("Error".to_owned(), message))
                        })
                    }),
                    None => handler(|_arg: Wire| {
                        Box::pin(async move {
                            Err((
                                "NotImplemented".to_owned(),
                                "provider runtime headers need an App-level Host".to_owned(),
                            ))
                        })
                    }),
                },
            ),
            (
                "initializeWorkspace",
                handler({
                    let instances = instances.clone();
                    let options = Arc::clone(&options);
                    move |arg: Wire| {
                        let instances = instances.clone();
                        let options = Arc::clone(&options);
                        Box::pin(async move {
                            // workspace 预热：TS `agentService.initialize` 返回 agent 可用性与协议常量，
                            // 首屏不因 agent 缺失崩溃（失败也回 `available:false`）。Rust Host 同构：
                            // 建/复用实例后等运行时报到；`transportKind` 固定为 "stdio"。
                            let target = crate::forward::runtime_target(params_of(arg))?;
                            let workspace_key = target.identity.clone();
                            let available = match instances
                                .get(&target.identity, || options(&target.descriptor))
                                .await
                            {
                                Ok(instance) => {
                                    instance
                                        .wait_ready(std::time::Duration::from_secs(30))
                                        .await
                                }
                                Err(_) => false,
                            };
                            Ok(Wire::Object(json!({
                                "available": available,
                                "workspaceKey": workspace_key,
                                "protocolName": "ESCode Protocol",
                                "protocolVersion": 1,
                                "transportKind": "stdio",
                            })))
                        })
                    }
                }),
            ),
            (
                "readWorkspacePresentation",
                handler({
                    let instances = instances.clone();
                    let options = Arc::clone(&options);
                    move |arg: Wire| {
                        let instances = instances.clone();
                        let options = Arc::clone(&options);
                        Box::pin(async move {
                            // workspace 展示面：TS agent 层先读 `runtime/capabilities` 再带
                            // `includeExecutionCapabilities` 读 `workspace/readPresentation`，这里同序转发。
                            // 已登记差异：TS 读取前会同步账号 provider 配置（providers/account 域未迁移）。
                            let params = params_of(arg);
                            let capabilities = runtime_call(
                                &instances,
                                &options,
                                "runtime/capabilities",
                                params.clone(),
                                Extract::Result,
                            )
                            .await
                            .map_err(<(String, String)>::from)?;
                            let Wire::Object(capabilities) = capabilities else {
                                return Err((
                                    "Fatal".to_owned(),
                                    "runtime/capabilities answer is not an object".to_owned(),
                                ));
                            };
                            let extra = (capabilities["workspaceExecutionCapabilities"] == true)
                                .then_some(("includeExecutionCapabilities", Value::Bool(true)));
                            runtime_call_with(
                                &instances,
                                &options,
                                "workspace/readPresentation",
                                params,
                                extra,
                                Extract::Result,
                            )
                            .await
                            .map_err(Into::into)
                        })
                    }
                }),
            ),
        ],
    )
}
