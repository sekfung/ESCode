//! `broadcast` channel（本地模式；docs/specs/rust-u2-user-config-services.md §3）。

use std::sync::Arc;

use escode_host_services::broadcast::Broadcast;
use futures_util::stream;
use serde_json::Value;
use tokio::sync::broadcast::error::RecvError;

use crate::channel::{Channel, EventHandler, EventStream};
use crate::session::{MethodHandler, RpcError, handler_rpc};
use crate::wire::Wire;

fn first_arg(arg: Wire) -> Value {
    match arg.into_args().into_iter().next() {
        Some(Wire::Str(text)) => Value::String(text),
        Some(Wire::Object(value)) => value,
        _ => Value::Null,
    }
}

fn method<F>(hub: &Arc<Broadcast>, run: F) -> MethodHandler
where
    F: Fn(&Broadcast, Value) -> Result<Wire, RpcError> + Send + Sync + 'static,
{
    let hub = Arc::clone(hub);
    let run = Arc::new(run);
    handler_rpc(move |arg: Wire| {
        let (hub, run) = (Arc::clone(&hub), Arc::clone(&run));
        Box::pin(async move { run(&hub, first_arg(arg)) })
    })
}

fn lease_of(value: &Value) -> (String, String) {
    (
        value["key"].as_str().unwrap_or_default().to_owned(),
        value["token"].as_str().unwrap_or_default().to_owned(),
    )
}

pub(crate) fn broadcast_channel(hub: Arc<Broadcast>) -> Channel {
    let on_message: EventHandler = {
        let hub = Arc::clone(&hub);
        Arc::new(move |_arg: Wire| {
            let stream: EventStream =
                Box::pin(stream::unfold(hub.subscribe(), |mut receiver| async move {
                    loop {
                        match receiver.recv().await {
                            Ok(message) => return Some((Wire::Object(message), receiver)),
                            // 消费过慢丢了旧消息：继续收后面的（广播是状态同步提示，不是日志）。
                            Err(RecvError::Lagged(_)) => continue,
                            Err(RecvError::Closed) => return None,
                        }
                    }
                }));
            Ok(stream)
        })
    };
    Channel::new(
        "broadcast",
        vec![
            (
                "send",
                method(&hub, |hub, message| {
                    hub.send(&message)
                        .map_err(|text| RpcError::new("Error", text))?;
                    Ok(Wire::Undefined)
                }),
            ),
            (
                "acquireClaim",
                method(&hub, |hub, key| {
                    Ok(Wire::Object(
                        hub.acquire_claim(key.as_str().unwrap_or_default()),
                    ))
                }),
            ),
            (
                "commitClaim",
                method(&hub, |hub, lease| {
                    let (key, token) = lease_of(&lease);
                    hub.commit_claim(&key, &token);
                    Ok(Wire::Undefined)
                }),
            ),
            (
                "releaseClaim",
                method(&hub, |hub, lease| {
                    let (key, token) = lease_of(&lease);
                    hub.release_claim(&key, &token);
                    Ok(Wire::Undefined)
                }),
            ),
            (
                "tryClaim",
                method(&hub, |hub, key| {
                    Ok(Wire::Object(Value::Bool(
                        hub.try_claim(key.as_str().unwrap_or_default()),
                    )))
                }),
            ),
        ],
    )
    .with_events(vec![("onMessage", on_message)])
}
