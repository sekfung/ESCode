//! App-Provided Tools 登记表单测（TS `McpUiAppToolRegistry` 对照语料）。
use super::*;
use serde_json::json;

fn tool(name: &str) -> Value {
    json!({"name":name,"inputSchema":{"type":"object","properties":{}}})
}

fn params(scope: &str, generation: u64, server: &str, tools: Vec<Value>) -> Value {
    json!({
        "sessionId":"session",
        "pluginId":"plugin",
        "serverName":server,
        "scopeId":scope,
        "generation":generation,
        "instance":{"runtimeId":"r","generation":generation,"token":scope,"appIdentity":"a"},
        "tools":tools,
    })
}

#[test]
fn register_exposes_model_names_and_replaces_older_generation() {
    let mut tools = AppTools::default();
    let first = tools
        .register("session", &params("scope", 1, "srv", vec![tool("lookup")]))
        .expect("register");
    assert_eq!(first["tools"][0]["modelName"], "app__srv__lookup");

    let second = tools
        .register("session", &params("scope", 2, "srv", vec![tool("search")]))
        .expect("register");
    assert_eq!(second["tools"][0]["modelName"], "app__srv__search");
    let definitions = tools.definitions("session");
    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0]["function"]["name"], "app__srv__search");
}

#[test]
fn only_the_latest_instance_per_server_is_exposed() {
    let mut tools = AppTools::default();
    tools
        .register("session", &params("scope-a", 1, "srv", vec![tool("a")]))
        .expect("first");
    tools
        .register("session", &params("scope-b", 1, "srv", vec![tool("b")]))
        .expect("second");
    let definitions = tools.definitions("session");
    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0]["function"]["name"], "app__srv__b");
}

#[test]
fn unregister_removes_the_instance_and_reports_tool_count() {
    let mut tools = AppTools::default();
    let params = params("scope", 1, "srv", vec![tool("a"), tool("b")]);
    tools.register("session", &params).expect("register");
    assert_eq!(tools.unregister("session", &params), 2);
    assert!(tools.definitions("session").is_empty());
    assert_eq!(tools.unregister("session", &params), 0);
}

#[tokio::test]
async fn claim_and_resolve_settle_a_prepared_call() {
    let mut tools = AppTools::default();
    tools
        .register("session", &params("scope", 1, "srv", vec![tool("run")]))
        .expect("register");
    let (call_id, delta, claim, settle) = tools
        .prepare_call("session", "app__srv__run", &json!({"x":1}))
        .expect("prepare");
    assert_eq!(delta["op"], "pluginUi.appToolCall");
    assert_eq!(delta["callId"], call_id);
    assert_eq!(delta["toolName"], "run");

    let claim_params =
        json!({"sessionId":"session","scopeId":"scope","generation":1,"callId":call_id});
    assert!(tools.claim("session", &claim_params));
    assert!(claim.await.is_ok(), "claim 信号必须唤醒执行方");

    let resolve_params = json!({
        "sessionId":"session","scopeId":"scope","generation":1,"callId":call_id,
        "result":{"content":[{"type":"text","text":"ok"}]}
    });
    assert!(tools.resolve("session", &resolve_params));
    let value = settle.await.expect("settle").expect("result");
    assert_eq!(value["content"][0]["text"], "ok");
}

#[test]
fn resolve_requires_a_matching_claimed_call() {
    let mut tools = AppTools::default();
    tools
        .register("session", &params("scope", 1, "srv", vec![tool("run")]))
        .expect("register");
    let (call_id, _, _claim, _settle) = tools
        .prepare_call("session", "app__srv__run", &json!({}))
        .expect("prepare");
    let resolve = json!({"sessionId":"session","scopeId":"scope","generation":1,"callId":call_id,"error":{"message":"no"}});
    assert!(!tools.resolve("session", &resolve), "未认领的调用不能回传");
    assert!(
        !tools.claim(
            "session",
            &json!({"sessionId":"session","scopeId":"other","generation":1,"callId":call_id})
        ),
        "跨实例认领必须拒绝"
    );
}

#[tokio::test]
async fn await_result_reports_page_not_running_and_cancellation() {
    let (claim_tx, claim_rx) = oneshot::channel();
    drop(claim_tx);
    let (_settle_tx, settle_rx) = oneshot::channel();
    let error = await_result(claim_rx, settle_rx, "srv", &CancellationToken::new())
        .await
        .expect_err("dropped claim");
    assert!(error.to_string().contains("app page is not running"));

    let cancel = CancellationToken::new();
    cancel.cancel();
    let (_claim_tx, claim_rx) = oneshot::channel();
    let (_settle_tx, settle_rx) = oneshot::channel();
    let error = await_result(claim_rx, settle_rx, "srv", &cancel)
        .await
        .expect_err("cancelled");
    assert!(error.to_string().contains("cancelled"));
}

#[tokio::test]
async fn await_result_forwards_page_errors() {
    let (claim_tx, claim_rx) = oneshot::channel();
    let (settle_tx, settle_rx) = oneshot::channel();
    claim_tx.send(()).expect("claim");
    settle_tx
        .send(Err("page failed".to_owned()))
        .expect("settle");
    let error = await_result(claim_rx, settle_rx, "srv", &CancellationToken::new())
        .await
        .expect_err("page error");
    assert_eq!(error.to_string(), "page failed");
}

#[test]
fn duplicate_resolve_is_idempotent() {
    let mut tools = AppTools::default();
    tools
        .register("session", &params("scope", 1, "srv", vec![tool("run")]))
        .expect("register");
    let (call_id, _, _claim, _settle) = tools
        .prepare_call("session", "app__srv__run", &json!({}))
        .expect("prepare");
    let claim = json!({"sessionId":"session","scopeId":"scope","generation":1,"callId":call_id});
    assert!(tools.claim("session", &claim));
    let resolve = json!({"sessionId":"session","scopeId":"scope","generation":1,"callId":call_id,"result":{"content":[]}});
    assert!(tools.resolve("session", &resolve));
    assert!(tools.resolve("session", &resolve), "重复回传保持幂等 true");
}
