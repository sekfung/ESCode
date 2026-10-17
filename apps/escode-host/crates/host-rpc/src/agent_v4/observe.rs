//! Bot group 的 Host 内部 V4 conversation 观察（TS `groupRuntimeSnapshot.ts` 的
//! `observeRuntime` 同构）：background 订阅 → 帧装配 → 首个满足条件的 payload。
//!
//! 观察器用独立的 `Base`（独立帧泵）与固定连接身份，不复用 Renderer 连接的订阅表；
//! 用后必须 unsubscribe，迟到 ACK 也要释放（超时覆盖 ACK）。

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::assembler::{AssemblyEvent, TopicWireFrameAssembler};
use super::base::{Base, Connection, conversation_subscribe_body};
use super::frames::TopicKind;
use super::scope::workspace_key;
use crate::forward::{RuntimePolicy, workspace_ref_json};
use crate::session::{ClientMode, RpcError};

const OBSERVE_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) async fn conversation_snapshot(
    base: &Arc<Base>,
    target: &Value,
) -> Result<Value, RpcError> {
    observe(base, target, |payload| {
        (payload["kind"] == "snapshot").then(|| payload["snapshot"].clone())
    })
    .await
}

async fn observe<F>(base: &Arc<Base>, target: &Value, select: F) -> Result<Value, RpcError>
where
    F: Fn(&Value) -> Option<Value>,
{
    let key = workspace_key(target);
    let session_id = target["sessionId"].as_str().unwrap_or_default();
    let topic = format!("conversation/{session_id}");
    let mut frames = base.hub.listen(&key, TopicKind::Conversation);
    let connection = Connection {
        connection_id: format!("host-observer-{}", uuid::Uuid::new_v4()),
        client_mode: ClientMode::DesktopContinuous,
        workflow_run_deltas: false,
    };
    let workspace = workspace_ref_json(
        target["workspacePath"].as_str().unwrap_or_default(),
        target["workspaceIdentity"].as_str(),
        target["remoteSessionId"].as_str(),
        &key,
    );
    let mut body = conversation_subscribe_body(target, &topic, &connection, workspace, None);
    body["visibility"] = Value::from("background");
    let subscribed = base
        .subscribe(target, RuntimePolicy::StartIfNeeded, body, Instant::now())
        .await?;
    let subscription_id = subscribed.result["ack"]["subscriptionId"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    if subscription_id.is_empty() {
        return Err(RpcError::new(
            "Error",
            "Group runtime snapshot returned no subscriptionId",
        ));
    }

    let result = tokio::time::timeout(OBSERVE_TIMEOUT, async {
        let mut assembler =
            TopicWireFrameAssembler::new(|value| value["payload"]["kind"].is_string());
        while let Some(frame) = frames.recv().await {
            if frame.topic != topic || frame.subscription_id != subscription_id {
                continue;
            }
            for event in assembler.accept(&frame.params, now_ms()) {
                match event {
                    AssemblyEvent::Complete { frame, .. } => {
                        if let Some(selected) = select(&frame["payload"]) {
                            return Ok(selected);
                        }
                    }
                    AssemblyEvent::Fault(fault) => {
                        return Err(RpcError::new("Error", fault.reason));
                    }
                }
            }
        }
        Err(RpcError::new(
            "Error",
            "Group runtime snapshot subscription closed",
        ))
    })
    .await;

    let _ = base
        .call(
            target,
            RuntimePolicy::StartIfNeeded,
            "v4/conversation/unsubscribe",
            json!({
                "topic": topic,
                "subscriptionId": subscription_id,
                "connectionId": connection.connection_id,
            }),
        )
        .await;
    match result {
        Ok(result) => result,
        Err(_) => Err(RpcError::new("Error", "Group runtime snapshot timed out")),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
