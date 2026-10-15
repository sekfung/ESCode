//! V4 作用域：hello / clientHello 握手（TS `createHello`、`initializeConversationV4`、`clientHelloSchema`）。

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use super::scope::{Scope, fault};
use crate::session::{ClientMode, RpcError};

/// TS `V4_WIRE_PROTOCOL_VERSION`。
const PROTOCOL_VERSION: u64 = 3;

impl Scope {
    /// `helloConversationV4`（TS `createHello`）。
    pub(crate) fn hello(&self) -> Result<Value, RpcError> {
        self.assert_open()?;
        self.lock().hello_issued = true;
        let continuous = self.client_mode == ClientMode::DesktopContinuous;
        let server_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
            });
        Ok(json!({
            "kind": "hello",
            "protocolVersion": PROTOCOL_VERSION,
            "connectionId": self.connection_id,
            "clientMode": self.client_mode.as_str(),
            "deliveryProfile": if continuous { "continuous" } else { "replayable" },
            "serverTime": server_time,
            "capabilities": {
                "nativeDialogs": continuous,
                "localTerminal": continuous,
                "binaryFrames": false,
                "compression": "none",
                "workspaceHookReview": true,
                "independentPlanState": true,
                "workflowRunDeltas": true,
            },
            "auth": {},
        }))
    }

    /// `initializeConversationV4(clientHello)`。
    pub(crate) fn initialize(&self, client_hello: &Value) -> Result<(), RpcError> {
        self.assert_open()?;
        if !self.lock().hello_issued {
            return Err(fault("fault.connection.helloRequired"));
        }
        let (client_id, deltas) = parse_client_hello(client_hello)?;
        let mut state = self.lock();
        if state
            .bound_client_id
            .as_ref()
            .is_some_and(|bound| *bound != client_id)
        {
            return Err(fault("fault.connection.clientChanged"));
        }
        state.bound_client_id = Some(client_id);
        state.workflow_run_deltas = deltas;
        state.handshake_complete = true;
        Ok(())
    }
}

/// TS `clientHelloSchema`（strict）+ `clientSupportsWorkflowRunDeltas`。
fn parse_client_hello(value: &Value) -> Result<(String, bool), RpcError> {
    let invalid =
        |detail: &str| RpcError::new("ZodError", format!("Invalid clientHello: {detail}"));
    let record = value
        .as_object()
        .ok_or_else(|| invalid("expected object"))?;
    const KEYS: [&str; 6] = [
        "kind",
        "protocolVersion",
        "clientId",
        "clientKind",
        "appVersion",
        "capabilities",
    ];
    if let Some(extra) = record.keys().find(|key| !KEYS.contains(&key.as_str())) {
        return Err(invalid(&format!("unrecognized key `{extra}`")));
    }
    if record.get("kind") != Some(&Value::from("clientHello")) {
        return Err(invalid("kind"));
    }
    if record.get("protocolVersion").and_then(Value::as_u64) != Some(PROTOCOL_VERSION) {
        return Err(invalid("protocolVersion"));
    }
    let client_id = record
        .get("clientId")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("clientId"))?
        .to_owned();
    if !record.get("appVersion").is_some_and(Value::is_string) {
        return Err(invalid("appVersion"));
    }
    if let Some(kind) = record.get("clientKind")
        && !matches!(
            kind.as_str(),
            Some("desktop" | "web" | "mobileRemote" | "mobileApp")
        )
    {
        return Err(invalid("clientKind"));
    }
    let mut deltas = false;
    if let Some(capabilities) = record.get("capabilities") {
        let capabilities = capabilities
            .as_object()
            .ok_or_else(|| invalid("capabilities"))?;
        for (key, flag) in capabilities {
            if !matches!(key.as_str(), "workspaceHookReviewUi" | "workflowRunDeltas") {
                return Err(invalid(&format!("unrecognized capability `{key}`")));
            }
            if !flag.is_boolean() {
                return Err(invalid(key));
            }
        }
        deltas = capabilities.get("workflowRunDeltas") == Some(&Value::Bool(true));
    }
    Ok((client_id, deltas))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_hello_is_strict_and_reads_the_delta_capability() {
        let hello = json!({"kind": "clientHello", "protocolVersion": 3, "clientId": "c1", "appVersion": "3.15.1",
            "clientKind": "desktop", "capabilities": {"workflowRunDeltas": true, "workspaceHookReviewUi": true}});
        assert_eq!(
            parse_client_hello(&hello).expect("valid"),
            ("c1".to_owned(), true)
        );
        let minimal = json!({"kind": "clientHello", "protocolVersion": 3, "clientId": "c1", "appVersion": "x"});
        assert_eq!(
            parse_client_hello(&minimal).expect("valid"),
            ("c1".to_owned(), false)
        );
        for invalid in [
            json!({"kind": "clientHello", "protocolVersion": 2, "clientId": "c1", "appVersion": "x"}),
            json!({"kind": "clientHello", "protocolVersion": 3, "clientId": "c1", "appVersion": "x", "extra": 1}),
            json!({"kind": "clientHello", "protocolVersion": 3, "clientId": "c1", "appVersion": "x", "capabilities": {"other": true}}),
            json!({"kind": "clientHello", "protocolVersion": 3, "clientId": "c1", "appVersion": "x", "clientKind": "tv"}),
            json!({"kind": "clientHello", "protocolVersion": 3, "appVersion": "x"}),
        ] {
            assert!(parse_client_hello(&invalid).is_err(), "{invalid}");
        }
    }
}
