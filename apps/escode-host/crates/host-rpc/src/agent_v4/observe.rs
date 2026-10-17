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
use crate::session::{ClientMode, EventStream, RpcError};
use crate::wire::Wire;

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

/// `escode-task.onDynamicConversationFrames`（docs/specs/bots-v4-stream.md）：持续的 background
/// 订阅，按序投递装配后的帧（首帧 snapshot）。fault 时退订并重订阅，下一帧是新 snapshot；
/// 事件流被丢弃（监听方 dispose / 连接关闭）后退订并结束泵。
pub(crate) fn conversation_frames(base: Arc<Base>, target: Value) -> EventStream {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    tokio::spawn(async move {
        while !tx.is_closed() {
            match pump_once(&base, &target, &tx).await {
                // 监听方已离开：正常结束。
                Ok(()) => break,
                // fault / 订阅失败 / 帧泵关闭：稍后重订阅，下一帧是新 snapshot。
                Err(_) => {
                    tokio::select! {
                        () = tokio::time::sleep(RESUBSCRIBE_DELAY) => {}
                        () = tx.closed() => break,
                    }
                }
            }
        }
    });
    Box::pin(futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|frame| (Wire::Object(frame), rx))
    }))
}

const RESUBSCRIBE_DELAY: Duration = Duration::from_millis(500);

/// 一次订阅的生命周期：`Ok` 表示监听方已离开，`Err` 表示需要重订阅（fault、订阅失败、帧泵关闭）。
async fn pump_once(
    base: &Arc<Base>,
    target: &Value,
    tx: &tokio::sync::mpsc::UnboundedSender<Value>,
) -> Result<(), RpcError> {
    let key = workspace_key(target);
    let session_id = target["sessionId"].as_str().unwrap_or_default();
    let topic = format!("conversation/{session_id}");
    let mut frames = base.hub.listen(&key, TopicKind::Conversation);
    let connection = Connection {
        connection_id: format!("host-frame-feed-{}", uuid::Uuid::new_v4()),
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
            "Conversation subscribe returned no subscriptionId",
        ));
    }
    let outcome = async {
        let mut assembler =
            TopicWireFrameAssembler::new(|value| value["payload"]["kind"].is_string());
        let mut seq: Option<u64> = None;
        loop {
            let frame = tokio::select! {
                frame = frames.recv() => frame,
                () = tx.closed() => return Ok(()),
            };
            let Some(frame) = frame else {
                return Err(RpcError::new("Error", "Conversation frame pump closed"));
            };
            if frame.topic != topic || frame.subscription_id != subscription_id {
                continue;
            }
            for event in assembler.accept(&frame.params, now_ms()) {
                match event {
                    AssemblyEvent::Complete { frame, .. } => {
                        // 装配器不校验 seq 连续：deltas 必须接在上一帧 toSeq 之后，否则视为 gap。
                        if frame["payload"]["kind"] == "deltas" && frame["fromSeq"].as_u64() != seq
                        {
                            return Err(RpcError::new("Error", "Conversation frame gap"));
                        }
                        seq = frame["toSeq"].as_u64();
                        if tx.send(frame).is_err() {
                            return Ok(());
                        }
                    }
                    AssemblyEvent::Fault(fault) => {
                        return Err(RpcError::new("Error", fault.reason));
                    }
                }
            }
        }
    }
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
    outcome
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
