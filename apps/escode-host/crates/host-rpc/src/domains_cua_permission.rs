//! `cua-permission` channel（TS `ServiceChannels.CuaPermission` = `"cua-permission"`）。
//!
//! Helper 生命周期只能由签名的 Electron Main 拥有（`docs/specs/rust-u2-cua-permission.md`）。
//! Host 不探测、不启动、不重启 Helper，只把位置参数整形后经控制通道转发给 Main：
//! `Renderer → host-rpc → MainBroker.mainRequest → Main`。

use std::sync::Arc;

use escode_host_control::main_broker::MainBroker;
use serde_json::{Map, Value, json};

use crate::session::{Channel, RpcError, handler_rpc};
use crate::wire::Wire;

const NO_MAIN: &str = "cua-permission requires Electron Main; Main did not declare mainServices.";

fn require_broker(broker: Option<Arc<MainBroker>>) -> Result<Arc<MainBroker>, RpcError> {
    broker.ok_or_else(|| RpcError::new("NotImplemented", NO_MAIN))
}

/// `getStatus(workspacePath, workspaceIdentity?, options?)`：
/// 只有第一个参数必填。
fn status_params(args: &[Wire]) -> Result<Value, RpcError> {
    let workspace_path = args
        .first()
        .and_then(Wire::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| RpcError::new("InvalidRequest", "workspacePath is required."))?;
    Ok(positional(workspace_path, args))
}

/// `restartHelper(workspacePath?, workspaceIdentity?, options?)`：三个参数都可选。
fn restart_params(args: &[Wire]) -> Value {
    let workspace_path = args
        .first()
        .and_then(Wire::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match workspace_path {
        Some(path) => positional(path, args),
        None => {
            let mut body = Map::new();
            // 第一个位置始终是 workspacePath（可能为空/undefined）；identity/options 从下标 1 开始。
            let start = usize::from(!args.is_empty());
            copy_tail(&mut body, args, start);
            Value::Object(body)
        }
    }
}

/// 缺省字段整个省略（runtime strict schema 拒绝 null；Main 侧同样按缺省处理）。
fn positional(workspace_path: &str, args: &[Wire]) -> Value {
    let mut body = Map::new();
    body.insert("workspacePath".into(), json!(workspace_path));
    copy_tail(&mut body, args, 1);
    Value::Object(body)
}

fn copy_tail(body: &mut Map<String, Value>, args: &[Wire], start: usize) {
    if let Some(identity) = args
        .get(start)
        .and_then(Wire::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        body.insert("workspaceIdentity".into(), json!(identity));
    }
    if let Some(options) = args
        .get(start + 1)
        .and_then(|value| value.clone().into_object())
    {
        body.insert("options".into(), options);
    }
}

fn method<F>(
    broker: Option<Arc<MainBroker>>,
    name: &'static str,
    build: F,
) -> crate::channel::MethodHandler
where
    F: Fn(&[Wire]) -> Result<Value, RpcError> + Send + Sync + 'static,
{
    let build = Arc::new(build);
    handler_rpc(move |arg: Wire| {
        let broker_ref = broker.clone();
        let build = Arc::clone(&build);
        let args = arg.into_args();
        Box::pin(async move {
            let params = build(&args)?;
            let broker = require_broker(broker_ref)?;
            broker
                .request(name, params)
                .await
                .map(Wire::Object)
                .map_err(|error| RpcError::new("Fatal", error))
        })
    })
}

pub(crate) fn cua_permission_channel(broker: Option<Arc<MainBroker>>) -> Channel {
    Channel::new(
        "cua-permission",
        vec![
            (
                "getStatus",
                method(broker.clone(), "cua-permission.getStatus", status_params),
            ),
            (
                "restartHelper",
                method(broker, "cua-permission.restartHelper", |args| {
                    Ok(restart_params(args))
                }),
            ),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<Wire> {
        values
            .iter()
            .map(|value| Wire::Str((*value).to_owned()))
            .collect()
    }

    #[test]
    fn get_status_requires_workspace_path() {
        assert!(status_params(&[]).is_err());
        assert!(status_params(&strings(&["  "])).is_err());
        assert!(status_params(&strings(&["/w"])).is_ok());
    }

    #[test]
    fn optional_fields_are_omitted_not_null() {
        let body = status_params(&strings(&["/w"])).expect("params");
        assert_eq!(body, json!({"workspacePath": "/w"}));

        let body = status_params(&[
            Wire::Str("/w".into()),
            Wire::Str(" identity ".into()),
            Wire::Object(json!({"includeFunctionalProbes": true})),
        ])
        .expect("params");
        assert_eq!(
            body,
            json!({
                "workspacePath": "/w",
                "workspaceIdentity": "identity",
                "options": {"includeFunctionalProbes": true},
            })
        );
    }

    #[test]
    fn restart_helper_accepts_no_workspace() {
        assert_eq!(restart_params(&[]), json!({}));
        assert_eq!(
            restart_params(&[Wire::Str("".into()), Wire::Str("id".into())]),
            json!({"workspaceIdentity": "id"})
        );
        assert_eq!(
            restart_params(&[
                Wire::Str("/w".into()),
                Wire::Undefined,
                Wire::Object(json!({"reason": "permission_granted"})),
            ]),
            json!({
                "workspacePath": "/w",
                "options": {"reason": "permission_granted"},
            })
        );
    }
}

#[cfg(test)]
mod main_broker_tests {
    use super::*;
    use escode_host_control::{ControlMessage, ControlWriter};

    #[tokio::test]
    async fn without_main_services_returns_not_implemented() {
        let handler = method(None, "cua-permission.getStatus", status_params);
        let error = handler(Wire::Array(vec![Wire::Str("/w".into())]))
            .await
            .expect_err("not implemented");
        assert_eq!(error.name, "NotImplemented");
        assert!(error.message.contains("mainServices"));
    }

    #[tokio::test]
    async fn get_status_forwards_params_and_returns_the_main_result() {
        let (writer, mut rx) = ControlWriter::channel(8);
        let broker = MainBroker::new(writer);
        let handler = method(
            Some(Arc::clone(&broker)),
            "cua-permission.getStatus",
            status_params,
        );
        let call = tokio::spawn(async move {
            handler(Wire::Array(vec![
                Wire::Str("/w".into()),
                Wire::Undefined,
                Wire::Object(json!({"includeFunctionalProbes": true})),
            ]))
            .await
        });
        let Some(ControlMessage::MainRequest {
            request_id,
            method: name,
            params,
        }) = rx.recv().await
        else {
            panic!("expected mainRequest");
        };
        assert_eq!(name, "cua-permission.getStatus");
        assert_eq!(
            params,
            json!({"workspacePath": "/w", "options": {"includeFunctionalProbes": true}})
        );
        let result = json!({"available": false, "reason": "idle", "idle": true});
        assert!(broker.resolve(&request_id, true, Some(result.clone()), None));
        assert_eq!(
            call.await.expect("task").expect("result"),
            Wire::Object(result)
        );
    }

    #[tokio::test]
    async fn main_error_becomes_fatal_and_missing_workspace_is_rejected_locally() {
        let (writer, mut rx) = ControlWriter::channel(8);
        let broker = MainBroker::new(writer);
        let handler = method(
            Some(Arc::clone(&broker)),
            "cua-permission.getStatus",
            status_params,
        );

        let error = handler(Wire::Array(Vec::new())).await.expect_err("invalid");
        assert_eq!(error.name, "InvalidRequest");
        assert!(matches!(
            rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));

        let call =
            tokio::spawn(async move { handler(Wire::Array(vec![Wire::Str("/w".into())])).await });
        let Some(ControlMessage::MainRequest { request_id, .. }) = rx.recv().await else {
            panic!("expected mainRequest");
        };
        assert!(broker.resolve(&request_id, false, None, Some("boom".into())));
        let error = call.await.expect("task").expect_err("fatal");
        assert_eq!(error.name, "Fatal");
        assert_eq!(error.message, "boom");
    }
}
