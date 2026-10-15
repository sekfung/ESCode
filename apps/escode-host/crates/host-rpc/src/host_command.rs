//! Host 侧 V4 命令信封与 ACK 收口（TS `packages/services/src/escode-agent/escodeV4HostCommand.ts`）。
//!
//! Host 发起的命令（重命名同步、任务写面、自动化等）用进程内稳定的 `host-services-<uuid v7>` 作
//! clientId（Host 重启 = 新提交端，幂等表以 commandId 为键不受影响），commandId 缺省 uuid v7。

use std::sync::OnceLock;

use serde_json::{Value, json};

use crate::session::RpcError;

/// TS `COMMANDS_REQUIRING_BASE_REVISION`。
const CAS_COMMANDS: [&str; 15] = [
    "applyFileRewind",
    "forkAssistant",
    "editUserQuery",
    "retryTurn",
    "setAssistantFeedback",
    "sendQueuedNow",
    "editQueueItem",
    "reorderQueueItem",
    "deleteQueueItem",
    "setAutoDrain",
    "switchModelConfig",
    "switchCollaborationMode",
    "setFollowupMode",
    "pauseGoal",
    "resumeGoal",
];

/// TS `ROW_TARGETING_COMMANDS`。
const ROW_TARGETING_COMMANDS: [&str; 6] = [
    "applyFileRewind",
    "forkAssistant",
    "editUserQuery",
    "retryTurn",
    "setAssistantFeedback",
    "setHighspeedMetrics",
];

/// TS `getHostV4ClientId`：Host 进程内稳定。
pub(crate) fn host_client_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| format!("host-services-{}", uuid::Uuid::now_v7()))
}

/// TS `CreateHostCommandEnvelopeInput`。
pub(crate) struct HostCommand<'a> {
    pub kind: &'a str,
    pub payload: Value,
    pub session_id: Option<&'a str>,
    pub command_id: Option<String>,
    pub client_id: Option<String>,
    pub base_revision: Option<i64>,
    pub base_log_epoch: Option<String>,
}

impl<'a> HostCommand<'a> {
    pub(crate) fn new(kind: &'a str, session_id: Option<&'a str>, payload: Value) -> Self {
        Self {
            kind,
            payload,
            session_id,
            command_id: None,
            client_id: None,
            base_revision: None,
            base_log_epoch: None,
        }
    }
}

/// TS `createHostCommandEnvelope`：CAS 命令缺 baseRevision、row target 命令缺 baseLogEpoch 就地失败。
pub(crate) fn envelope(command: HostCommand<'_>) -> Result<Value, RpcError> {
    if CAS_COMMANDS.contains(&command.kind) && command.base_revision.is_none() {
        return Err(RpcError::new(
            "Error",
            format!(
                "command {} 是 CAS 命令，必须携带 baseRevision（10-protocol-spec §6.4）",
                command.kind
            ),
        ));
    }
    if ROW_TARGETING_COMMANDS.contains(&command.kind)
        && command.base_log_epoch.as_deref().is_none_or(str::is_empty)
    {
        return Err(RpcError::new(
            "Error",
            format!(
                "command {} 是 row target 命令，必须携带 baseLogEpoch",
                command.kind
            ),
        ));
    }
    let issued_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        });
    let mut envelope = json!({
        "commandId": command.command_id.unwrap_or_else(|| uuid::Uuid::now_v7().to_string()),
        "clientId": command.client_id.unwrap_or_else(|| host_client_id().to_owned()),
        "sessionId": command.session_id,
    });
    if let Some(revision) = command.base_revision {
        envelope["baseRevision"] = Value::from(revision);
    }
    if let Some(epoch) = command.base_log_epoch.filter(|epoch| !epoch.is_empty()) {
        envelope["baseLogEpoch"] = Value::from(epoch);
    }
    envelope["type"] = Value::from(command.kind);
    envelope["payload"] = command.payload;
    envelope["issuedAt"] = Value::from(issued_at);
    Ok(envelope)
}

/// TS `assertV4CommandAckOk`：accepted / duplicate / noop 视为成功；其余抛
/// `ESCodeV4CommandRejectedError`（`code: "ESCODE_V4_COMMAND_REJECTED"`，文案与 TS 同形）。
pub(crate) fn assert_ack_ok(kind: &str, ack: &Value, context: &str) -> Result<(), RpcError> {
    let status = ack["status"].as_str().unwrap_or_default();
    if matches!(status, "accepted" | "duplicate" | "noop") {
        return Ok(());
    }
    Err(rejected(kind, ack, context))
}

fn rejected(kind: &str, ack: &Value, context: &str) -> RpcError {
    let status = ack["status"].as_str().unwrap_or_default();
    let reason = ack["reasonCode"]
        .as_str()
        .map(|code| format!(" ({code})"))
        .unwrap_or_default();
    let message = ack["message"]
        .as_str()
        .map(|text| format!(": {text}"))
        .unwrap_or_default();
    RpcError::new(
        "ESCodeV4CommandRejectedError",
        format!("v4 command {kind} {status}{reason}{message} — {context}"),
    )
    .with("code", Value::from("ESCODE_V4_COMMAND_REJECTED"))
}

/// TS `sendHostCasCommandV4`：baseRevision=0 探测，stale 即用 `revisionAtDecision` 重试（每次新 commandId）。
#[allow(dead_code)] // 任务写面（setModel/setMode 的 CAS 命令，S3b）接入后删除本行。
pub(crate) async fn send_cas<F, Fut>(
    send: F,
    kind: &str,
    payload: Value,
    session_id: &str,
    context: &str,
    max_attempts: usize,
) -> Result<Value, RpcError>
where
    F: Fn(Value) -> Fut,
    Fut: std::future::Future<Output = Result<Value, RpcError>>,
{
    let mut base_revision = 0_i64;
    let mut last: Option<Value> = None;
    for _ in 0..max_attempts {
        let mut command = HostCommand::new(kind, Some(session_id), payload.clone());
        command.base_revision = Some(base_revision);
        let ack = send(envelope(command)?).await?;
        if ack["status"] == "stale" {
            base_revision = ack["revisionAtDecision"].as_i64().unwrap_or(base_revision);
            last = Some(ack);
            continue;
        }
        assert_ack_ok(kind, &ack, context)?;
        return Ok(ack);
    }
    let ack = last.unwrap_or_else(
        || json!({"commandId": "", "status": "stale", "revisionAtDecision": base_revision}),
    );
    Err(rejected(
        kind,
        &ack,
        &format!("{context}（连续 stale，放弃重试）"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelopes_follow_the_ts_factory() {
        let built = envelope(HostCommand::new(
            "renameSession",
            Some("sess_1"),
            json!({"title": "t"}),
        ))
        .expect("plain command");
        assert_eq!(built["type"], "renameSession");
        assert_eq!(built["sessionId"], "sess_1");
        assert_eq!(built["clientId"], host_client_id());
        assert!(host_client_id().starts_with("host-services-"));
        assert_eq!(built["commandId"].as_str().unwrap().len(), 36);
        assert!(built.get("baseRevision").is_none());
        let cas = envelope(HostCommand::new("switchModelConfig", Some("s"), json!({})));
        assert!(cas.unwrap_err().message.contains("CAS"));
        let mut row = HostCommand::new("retryTurn", Some("s"), json!({}));
        row.base_revision = Some(1);
        assert!(envelope(row).unwrap_err().message.contains("baseLogEpoch"));
    }

    #[test]
    fn ack_statuses_map_like_ts() {
        for ok in ["accepted", "duplicate", "noop"] {
            assert!(assert_ack_ok("x", &json!({"status": ok}), "ctx").is_ok());
        }
        let error = assert_ack_ok(
            "renameSession",
            &json!({"status": "rejected", "reasonCode": "guard.x", "message": "nope"}),
            "session=s",
        )
        .unwrap_err();
        assert_eq!(error.name, "ESCodeV4CommandRejectedError");
        assert_eq!(
            error.message,
            "v4 command renameSession rejected (guard.x): nope — session=s"
        );
        assert_eq!(error.extra["code"], "ESCODE_V4_COMMAND_REJECTED");
    }

    #[tokio::test]
    async fn cas_retries_stale_with_the_decision_revision() {
        let attempts = std::sync::Mutex::new(Vec::new());
        let ack = send_cas(
            |envelope| {
                let revision = envelope["baseRevision"].as_i64().unwrap();
                attempts.lock().unwrap().push(revision);
                async move {
                    Ok(if revision == 0 {
                        json!({"status": "stale", "revisionAtDecision": 7})
                    } else {
                        json!({"status": "accepted", "revisionAtDecision": 8})
                    })
                }
            },
            "switchModelConfig",
            json!({}),
            "s",
            "ctx",
            4,
        )
        .await
        .expect("accepted after one stale");
        assert_eq!(ack["status"], "accepted");
        assert_eq!(*attempts.lock().unwrap(), vec![0, 7]);
    }
}
