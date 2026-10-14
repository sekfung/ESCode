use super::*;

fn broker() -> Broker {
    Broker {
        socket: String::new(),
        token: "a".repeat(64),
        host: Arc::default(),
        sessions: Mutex::default(),
        candidates: Mutex::default(),
        connections: Mutex::default(),
    }
}

#[test]
fn rejects_wrong_token_and_subagent_scope() {
    let broker = broker();
    let good = json!({"token": "a".repeat(64), "runtimeScope": "main"});
    assert!(broker.authorize(&good).is_ok());
    let wrong = json!({"token": "b".repeat(64), "runtimeScope": "main"});
    assert!(
        broker
            .authorize(&wrong)
            .unwrap_err()
            .to_string()
            .contains("not authorized")
    );
    let short = json!({"token": "a", "runtimeScope": "main"});
    assert!(broker.authorize(&short).is_err());
    let subagent = json!({"token": "a".repeat(64), "runtimeScope": "subagent"});
    assert_eq!(
        broker.authorize(&subagent).unwrap_err().to_string(),
        SUBAGENT_MESSAGE
    );
}

#[test]
fn unknown_session_is_rejected_before_reaching_the_host() {
    let broker = broker();
    assert!(broker.context("sess_unknown", None).is_err());
    broker.remember("sess_1", &json!({"workspace_key": "k", "workspace_path": "p", "client_mode": "web-remote-replayable", "turn_id": "t1"}));
    let params = broker.context("sess_1", None).unwrap();
    assert_eq!(params["workspaceKey"], "k");
    assert_eq!(params["clientMode"], "web-remote-replayable");
    assert_eq!(params["turnId"], "t1");
    assert_eq!(params["sessionContext"], "live");
}
