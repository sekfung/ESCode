use super::*;
use tokio::io::AsyncWriteExt;

const TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn broker(host: Arc<OnceLock<EventSink>>) -> Arc<SerialBroker> {
    Arc::new(SerialBroker { socket: String::new(), token: TOKEN.into(), host, sessions: Mutex::default() })
}

fn request(extra: Value) -> Value {
    let mut base = json!({
        "id": uuid::Uuid::new_v4().to_string(),
        "token": TOKEN,
        "runtimeScope": "main",
        "sessionId": "sess_1",
        "op": "list",
        "args": {},
    });
    for (key, value) in extra.as_object().unwrap() {
        base[key] = value.clone();
    }
    base
}

/// 伪造 Host：从事件通道取出 HostRequest 并记录 (method, params)；respond 返回 None 时持有 reply 不应答（模拟长等待）。
type HostAnswer = Option<Result<Value, (i64, String, Value)>>;
type Seen = Arc<Mutex<Vec<(String, Value)>>>;

fn fake_host(
    respond: impl Fn(&str, &Value) -> HostAnswer + Send + 'static,
) -> (Arc<OnceLock<EventSink>>, Seen) {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<crate::contract::RunEvent>(16);
    let host = Arc::new(OnceLock::new());
    let _ = host.set(EventSink { session_id: "rust-host-channel".into(), run_id: "run".into(), tx });
    let seen = Arc::new(Mutex::new(vec![]));
    let record = seen.clone();
    tokio::spawn(async move {
        let mut held = vec![];
        while let Some(event) = rx.recv().await {
            if let Event::HostRequest { method, params, reply } = event.event {
                record.lock().unwrap().push((method.clone(), params.clone()));
                match respond(&method, &params) {
                    Some(answer) => {
                        let _ = reply.send(answer.map(|v| v.to_string()));
                    }
                    None => held.push(reply),
                }
            }
        }
    });
    (host, seen)
}

async fn roundtrip(broker: Arc<SerialBroker>, payload: Value) -> Value {
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let serving = tokio::spawn(broker.serve(server));
    client.write_all(format!("{payload}\n").as_bytes()).await.unwrap();
    let mut response = String::new();
    client.read_to_string(&mut response).await.unwrap();
    serving.await.unwrap();
    serde_json::from_str(response.trim()).unwrap()
}

#[test]
fn rejects_wrong_token_and_subagent_scope() {
    let broker = broker(Arc::default());
    assert!(broker.authorize(&request(json!({}))).is_ok());
    let wrong = broker.authorize(&request(json!({"token": "b".repeat(64)}))).unwrap_err();
    assert_eq!(wrong.code, "unauthorized");
    let subagent = broker.authorize(&request(json!({"runtimeScope": "subagent"}))).unwrap_err();
    assert_eq!(subagent.code, "unavailable");
}

#[test]
fn builds_host_params_from_remembered_session_and_rejects_unknown_ones() {
    let broker = broker(Arc::default());
    assert_eq!(broker.params(&request(json!({}))).unwrap_err().code, "unavailable");
    broker.remember(
        "sess_1",
        &json!({"workspace_key": "identity-1", "workspace_path": "C:/work", "workspace_identity": "identity-1", "turn_id": "t1"}),
    );
    let (method, params) = broker.params(&request(json!({"op": "write", "args": {"data": "AT"}}))).unwrap();
    assert_eq!(method, "interaction/serialWrite");
    assert_eq!(params["workspaceKey"], "identity-1");
    assert_eq!(params["workspacePath"], "C:/work");
    assert_eq!(params["workspaceIdentity"], "identity-1");
    assert_eq!(params["turnId"], "t1");
    assert_eq!(params["args"], json!({"data": "AT"}));
    assert!(params["requestId"].as_str().is_some());
    assert_eq!(broker.params(&request(json!({"op": "format"}))).unwrap_err().code, "invalidInput");
    assert_eq!(broker.params(&request(json!({"args": null}))).unwrap_err().code, "invalidInput");
}

#[test]
fn host_error_keeps_tool_error_code_from_data() {
    assert_eq!(
        host_error("Serial port is not open".into(), &json!({"code": "notOpen"})),
        BrokerError::new("notOpen", "Serial port is not open"),
    );
    assert_eq!(host_error("boom".into(), &Value::Null).code, "io");
}

#[tokio::test]
async fn forwards_request_and_returns_host_result() {
    let (host, seen) = fake_host(|_, _| Some(Ok(json!({"bytes": 4, "seq": 3}))));
    let broker = broker(host);
    broker.remember("sess_1", &json!({"workspace_key": "k", "workspace_path": "p"}));
    let payload = request(json!({"op": "write", "args": {"data": "AT"}}));
    let response = roundtrip(broker, payload.clone()).await;
    assert_eq!(response, json!({"id": payload["id"], "ok": true, "result": {"bytes": 4, "seq": 3}}));
    assert_eq!(seen.lock().unwrap()[0].0, "interaction/serialWrite");
}

#[tokio::test]
async fn host_failure_is_returned_with_its_code() {
    let (host, _) = fake_host(|_, _| Some(Err((-32010, "busy now".into(), json!({"code": "busy"})))));
    let broker = broker(host);
    broker.remember("sess_1", &json!({"workspace_key": "k", "workspace_path": "p"}));
    let response = roundtrip(broker, request(json!({}))).await;
    assert_eq!(response["ok"], false);
    assert_eq!(response["error"], json!({"code": "busy", "message": "busy now"}));
}

#[tokio::test]
async fn abandoned_wait_for_sends_cancel_to_the_host() {
    let (host, seen) = fake_host(|method, _| (method != "interaction/serialWaitFor").then(|| Ok(json!({}))));
    let broker = broker(host);
    broker.remember("sess_1", &json!({"workspace_key": "k", "workspace_path": "p"}));
    let payload = request(json!({"op": "waitFor", "args": {"pattern": "READY"}}));
    // Host 不应答 waitFor；超时丢弃 execute 等价于 serve 中对端断开的分支。
    let abandoned = tokio::time::timeout(std::time::Duration::from_millis(50), broker.execute(&payload)).await;
    assert!(abandoned.is_err());
    for _ in 0..100 {
        if seen.lock().unwrap().iter().any(|(m, _)| m == CANCEL_METHOD) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let seen = seen.lock().unwrap();
    let wait = &seen.iter().find(|(m, _)| m == "interaction/serialWaitFor").unwrap().1;
    let cancel = &seen.iter().find(|(m, _)| m == CANCEL_METHOD).expect("cancel sent").1;
    assert_eq!(cancel, &json!({"sessionId": "sess_1", "targetRequestId": wait["requestId"]}));
}

#[tokio::test]
async fn completed_requests_do_not_send_cancel() {
    let (host, seen) = fake_host(|_, _| Some(Ok(json!({"matched": false}))));
    let broker = broker(host);
    broker.remember("sess_1", &json!({"workspace_key": "k", "workspace_path": "p"}));
    broker.execute(&request(json!({"op": "waitFor", "args": {"pattern": "x"}}))).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert!(seen.lock().unwrap().iter().all(|(m, _)| m != CANCEL_METHOD));
}
