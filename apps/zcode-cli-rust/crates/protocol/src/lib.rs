use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// `\\?\C:\x` → `C:\x`，`\\?\UNC\srv\share\x` → `\\srv\share\x`；其他 verbatim 形态
/// （如 `\\?\Volume{..}`）没有等价的普通路径，返回 None。Node `fs.realpath` 不产生 verbatim 前缀，
/// Rust `canonicalize` 会产生；凡是把路径写进协议或与 Host 比较的地方都要归一。
pub fn simplify_verbatim(path: &str) -> Option<String> {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        return Some(format!(r"\\{rest}"));
    }
    let rest = path.strip_prefix(r"\\?\")?;
    let bytes = rest.as_bytes();
    (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':').then(|| rest.to_owned())
}

#[cfg(test)]
mod simplify_verbatim_tests {
    use super::simplify_verbatim;

    #[test]
    fn strips_verbatim_prefix_like_node_realpath() {
        assert_eq!(simplify_verbatim(r"\\?\C:\a\b").as_deref(), Some(r"C:\a\b"));
        assert_eq!(
            simplify_verbatim(r"\\?\UNC\srv\share\x").as_deref(),
            Some(r"\\srv\share\x")
        );
        assert_eq!(simplify_verbatim(r"\\?\Volume{1}\x"), None);
        assert_eq!(simplify_verbatim(r"C:\plain"), None);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttachmentRef {
    pub r#ref: String,
    pub file_name: String,
    pub mime: String,
    pub bytes: u64,
    pub preview_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum RequestId {
    Text(String),
    Number(i64),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: Option<RequestId>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
    pub trace: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Command {
    pub command_id: String,
    pub client_id: String,
    pub session_id: Option<String>,
    pub base_revision: Option<u64>,
    pub base_log_epoch: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
    pub payload: Value,
    pub issued_at: f64,
}

impl Command {
    pub fn key(&self) -> String {
        serde_json::to_string(&(self.session_id.as_deref(), &self.command_id)).unwrap()
    }

    pub fn ack(&self, status: &str, revision: u64, reason: Option<&str>) -> Value {
        let mut ack =
            json!({"commandId": self.command_id, "status": status, "revisionAtDecision": revision});
        if let Some(reason) = reason {
            ack["reasonCode"] = reason.into();
        }
        ack
    }
}

pub fn rpc_error(id: &Option<RequestId>, code: i32, message: &str) -> Value {
    json!({"id": id.clone().unwrap_or(RequestId::Text("invalid-message".into())), "error":{"code":code,"message":message}})
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelSelection {
    pub provider_id: String,
    pub model_id: String,
    pub options: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommandAck {
    pub command_id: String,
    pub status: AckStatus,
    pub revision_at_decision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AckStatus {
    Accepted,
    Duplicate,
    Stale,
    Rejected,
    Noop,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EventEnvelope {
    pub trace_id: String,
    pub session_id: String,
    pub run_id: Option<String>,
    pub turn_id: Option<String>,
    pub sequence: u64,
    pub kind: String,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub session_id: String,
    pub revision: u64,
    pub log_epoch: String,
    pub state: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Capability {
    pub id: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_and_event_round_trip() {
        let command = Command {
            command_id: "cmd-1".into(),
            client_id: "client-1".into(),
            session_id: Some("session-1".into()),
            base_revision: Some(3),
            base_log_epoch: Some("epoch".into()),
            kind: "sendText".into(),
            payload: serde_json::json!({"text":"hello"}),
            issued_at: 1.0,
        };
        let encoded = serde_json::to_string(&command).unwrap();
        assert_eq!(serde_json::from_str::<Command>(&encoded).unwrap(), command);

        let event = EventEnvelope {
            trace_id: "trace".into(),
            session_id: "session-1".into(),
            run_id: Some("run".into()),
            turn_id: Some("turn".into()),
            sequence: 4,
            kind: "assistantText".into(),
            payload: serde_json::json!({"text":"hello"}),
        };
        assert_eq!(
            serde_json::from_str::<EventEnvelope>(&serde_json::to_string(&event).unwrap()).unwrap(),
            event
        );
    }
}
