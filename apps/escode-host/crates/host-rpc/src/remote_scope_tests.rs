//! `remote_scope` 单测（spec §10.11 验收 1）：方法转发帧、事件订阅与销毁、错误 code 透传、
//! `Wire`↔`WireValue` 往返。转发层不解释业务语义，这里也不启动真实 transport。

use std::time::Duration;

use futures_util::StreamExt;
use serde_json::json;
use tokio::sync::mpsc;

use escode_host_core::rpc_serialization::WireValue;
use escode_host_services::source_server_client::{
    RESPONSE_EVENT_FIRE, RESPONSE_INITIALIZE, RESPONSE_PROMISE_ERROR, RESPONSE_PROMISE_SUCCESS,
    RemoteChannelClient,
};

use super::*;
use crate::channel::EventStream;
use crate::session::RpcResult;
use crate::wire::Wire;

const CHANNEL: &str = "window-project-changes";

/// 已 `Initialize`（广播 `channels`）的客户端 + 出站帧接收端。
fn initialized_client(
    channels: &[&str],
) -> (Arc<RemoteChannelClient>, mpsc::UnboundedReceiver<Vec<u8>>) {
    let (outbound, receiver) = mpsc::unbounded_channel();
    let client = Arc::new(RemoteChannelClient::new(outbound));
    let mut payload = WireValue::Array(vec![WireValue::Int(RESPONSE_INITIALIZE)]).encode();
    WireValue::Object(json!({ "channels": channels })).encode_into(&mut payload);
    client.handle_frame(&payload).expect("initialize frame");
    (client, receiver)
}

/// 取一条出站请求的 `(header, arg)`。
async fn next_request(receiver: &mut mpsc::UnboundedReceiver<Vec<u8>>) -> (WireValue, WireValue) {
    let frame = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
        .await
        .expect("request frame within timeout")
        .expect("outbound channel open");
    let mut frames = crate::frame::FrameReader::new();
    frames.push(&frame);
    let payload = frames
        .next_frame()
        .expect("frame parses")
        .expect("complete frame");
    let (header, used) = WireValue::decode(&payload).expect("header decodes");
    let (arg, _) = WireValue::decode(&payload[used..]).expect("arg decodes");
    (header, arg)
}

/// 远端响应帧：`[kind, id]` + body。
fn response_frame(kind: i32, id: i32, body: WireValue) -> Vec<u8> {
    let mut payload = WireValue::Array(vec![WireValue::Int(kind), WireValue::Int(id)]).encode();
    body.encode_into(&mut payload);
    payload
}

fn int_arg(value: &WireValue) -> i32 {
    match value {
        WireValue::Int(value) => *value,
        other => panic!("expected integer in header, got {other:?}"),
    }
}

fn string_arg(value: &WireValue) -> String {
    match value {
        WireValue::String(value) => value.clone(),
        other => panic!("expected string in header, got {other:?}"),
    }
}

fn channel_named(client: Arc<RemoteChannelClient>, name: &'static str) -> crate::channel::Channel {
    forwarded_channels(client)
        .into_iter()
        .find(|channel| channel.name == name)
        .unwrap_or_else(|| panic!("{name} must be forwarded"))
}

/// 方法 → Promise 帧；远端成功响应按 `WireValue`→`Wire` 转回。
#[tokio::test]
async fn forwards_method_calls_as_promise_frames() {
    let (client, mut outbound) = initialized_client(&[CHANNEL]);
    let channel = channel_named(Arc::clone(&client), CHANNEL);
    let handler = channel
        .handler("getCapabilities")
        .expect("method forwarded");

    let call = tokio::spawn(async move { handler(Wire::Array(vec![])).await });
    let (header, arg) = next_request(&mut outbound).await;
    let WireValue::Array(items) = &header else {
        panic!("request header is an array: {header:?}");
    };
    assert_eq!(int_arg(&items[0]), 100, "Promise 请求");
    assert_eq!(string_arg(&items[2]), CHANNEL);
    assert_eq!(string_arg(&items[3]), "getCapabilities");
    assert_eq!(
        arg,
        WireValue::Array(vec![]),
        "实参按 ProxyChannel 数组转发"
    );

    client
        .handle_frame(&response_frame(
            RESPONSE_PROMISE_SUCCESS,
            int_arg(&items[1]),
            WireValue::Object(json!({ "git": true })),
        ))
        .expect("success frame");
    let result: RpcResult = call.await.expect("handler task");
    assert_eq!(
        result.expect("call succeeds"),
        Wire::Object(json!({ "git": true }))
    );
}

/// 事件 → EventListen 帧；流丢弃时发 EventDispose（远端订阅不泄漏）。
#[tokio::test]
async fn forwards_events_and_disposes_the_remote_listener() {
    let (client, mut outbound) = initialized_client(&[CHANNEL]);
    let channel = channel_named(Arc::clone(&client), CHANNEL);
    let handler = channel.event("onDidChange").expect("event forwarded");
    let mut stream: EventStream = handler(Wire::Array(vec![])).expect("listener stream");

    let fire = async {
        let (header, _arg) = next_request(&mut outbound).await;
        let WireValue::Array(items) = &header else {
            panic!("event header is an array: {header:?}");
        };
        assert_eq!(int_arg(&items[0]), 102, "EventListen 请求");
        assert_eq!(string_arg(&items[2]), CHANNEL);
        assert_eq!(string_arg(&items[3]), "onDidChange");
        let id = int_arg(&items[1]);
        client
            .handle_frame(&response_frame(
                RESPONSE_EVENT_FIRE,
                id,
                WireValue::Object(json!({ "generation": 7 })),
            ))
            .expect("event fire frame");
        id
    };
    let (value, id) = tokio::join!(
        tokio::time::timeout(Duration::from_secs(2), stream.next()),
        fire
    );
    assert_eq!(
        value.expect("event within timeout"),
        Some(Wire::Object(json!({ "generation": 7 })))
    );

    drop(stream);
    let (header, _) = next_request(&mut outbound).await;
    let WireValue::Array(items) = &header else {
        panic!("dispose header is an array: {header:?}");
    };
    assert_eq!(int_arg(&items[0]), 103, "EventDispose 请求");
    assert_eq!(int_arg(&items[1]), id, "销毁的是同一个订阅");
}

/// EventListen 必须在 handler 返回时就进入远端出站队列。
///
/// macOS CI（run `41277156616`）上 `v4_frames_match_the_node_remote_oracle` 曾 5s 无帧超时：惰性建流让
/// 紧随 listen 的 `subscribeConversationV4` 先到远端，远端立即投递、不重发的帧丢失。
/// 这里不 poll 事件流，直接发一个 `call`，出站队列的第一帧必须是 EventListen。
#[tokio::test]
async fn event_listen_is_enqueued_before_a_following_call() {
    let (client, mut outbound) = initialized_client(&[CHANNEL]);
    let channel = channel_named(Arc::clone(&client), CHANNEL);
    let handler = channel.event("onDidChange").expect("event forwarded");
    let _stream: EventStream = handler(Wire::Array(vec![])).expect("listener stream");

    let call = tokio::spawn(async move {
        channel
            .handler("getCapabilities")
            .expect("method forwarded")(Wire::Array(vec![]))
        .await
    });
    let (header, _arg) = next_request(&mut outbound).await;
    let WireValue::Array(items) = &header else {
        panic!("first request is an array: {header:?}");
    };
    assert_eq!(int_arg(&items[0]), 102, "首帧必须是 EventListen");
    assert_eq!(string_arg(&items[3]), "onDidChange");
    let (header, _arg) = next_request(&mut outbound).await;
    let WireValue::Array(items) = &header else {
        panic!("second request is an array: {header:?}");
    };
    assert_eq!(int_arg(&items[0]), 100, "随后才是 Promise 请求");
    call.abort();
}

/// 远端错误保留 `name`/`message`，`data.code` 提到顶层（TS 客户端按 `code` 识别）。
#[tokio::test]
async fn preserves_remote_error_name_message_and_code() {
    let (client, mut outbound) = initialized_client(&[CHANNEL]);
    let channel = channel_named(Arc::clone(&client), CHANNEL);
    let handler = channel
        .handler("getCapabilities")
        .expect("method forwarded");

    let call = tokio::spawn(async move { handler(Wire::Array(vec![])).await });
    let (header, _) = next_request(&mut outbound).await;
    let WireValue::Array(items) = &header else {
        panic!("request header is an array: {header:?}");
    };
    client
        .handle_frame(&response_frame(
            RESPONSE_PROMISE_ERROR,
            int_arg(&items[1]),
            WireValue::Object(json!({
                "name": "InvalidRequest",
                "message": "workspace is not ready",
                "code": "ESCODE_WORKSPACE_NOT_READY",
            })),
        ))
        .expect("error frame");

    let error = call
        .await
        .expect("handler task")
        .expect_err("call must fail");
    assert_eq!(error.name, "InvalidRequest");
    assert_eq!(error.message, "workspace is not ready");
    assert_eq!(
        error.extra.get("code"),
        Some(&json!("ESCODE_WORKSPACE_NOT_READY"))
    );
}

/// 远端未广播的 channel 不创建转发入口（不猜远端能力）。
#[test]
fn only_advertised_channels_are_forwarded() {
    let (client, _outbound) = initialized_client(&[CHANNEL]);
    let names: Vec<&str> = forwarded_channels(client)
        .iter()
        .map(|channel| channel.name)
        .collect();
    assert_eq!(names, vec![CHANNEL]);
}

/// S4-g：Provider Provisioning target 只允许 Host 调用，远端即使广播也不转发给 Renderer。
#[test]
fn provisioning_target_is_never_forwarded() {
    let (client, _outbound) = initialized_client(&["provider-provisioning-target"]);
    let names: Vec<&str> = forwarded_channels(client)
        .iter()
        .map(|channel| channel.name)
        .collect();
    assert!(names.is_empty(), "private channel leaked: {names:?}");
}

/// S4-f：远端广播 `source-server-workspace` 后，Host 原样转发方法与参数（owner 仍在远端）。
#[tokio::test]
async fn forwards_source_server_workspace_requests() {
    const WORKSPACE: &str = "source-server-workspace";
    let (client, mut outbound) = initialized_client(&[WORKSPACE]);
    let channel = channel_named(Arc::clone(&client), WORKSPACE);
    let handler = channel
        .handler("resolveWorkspace")
        .expect("resolveWorkspace forwarded");

    let call = tokio::spawn(async move {
        handler(Wire::Array(vec![Wire::Object(json!({
            "workspacePath": "/remote/workspace"
        }))]))
        .await
    });
    let (header, arg) = next_request(&mut outbound).await;
    let WireValue::Array(items) = &header else {
        panic!("request header is an array: {header:?}");
    };
    assert_eq!(string_arg(&items[2]), WORKSPACE);
    assert_eq!(string_arg(&items[3]), "resolveWorkspace");
    assert_eq!(
        arg,
        WireValue::Array(vec![WireValue::Object(json!({
            "workspacePath": "/remote/workspace"
        }))]),
        "参数按 ProxyChannel 数组原样转发"
    );
    client
        .handle_frame(&response_frame(
            RESPONSE_PROMISE_SUCCESS,
            int_arg(&items[1]),
            WireValue::Object(json!({
                "canonicalWorkspaceKey": "remote:server:fixture:/remote/workspace",
                "mergedKeys": []
            })),
        ))
        .expect("success frame");
    let result: RpcResult = call.await.expect("handler task");
    assert_eq!(
        result.expect("call succeeds"),
        Wire::Object(json!({
            "canonicalWorkspaceKey": "remote:server:fixture:/remote/workspace",
            "mergedKeys": []
        }))
    );
}

/// `Wire`↔`WireValue` 往返：含 100 KB 字节串（二进制载荷不经过 JSON）。
#[test]
fn wire_value_round_trip_preserves_large_binary_payloads() {
    let bytes = vec![0xA5_u8; 100 * 1024];
    let value = Wire::Bytes {
        vs: false,
        data: bytes.clone(),
    };
    let wire_value = WireValue::from(value.clone());
    assert_eq!(Wire::from(wire_value), value, "字节串原样往返");

    let vs = Wire::Bytes {
        vs: true,
        data: bytes,
    };
    assert_eq!(
        Wire::from(WireValue::from(vs.clone())),
        vs,
        "VSBuffer 标签保留"
    );
}

/// 首连请求携带 caller key（取消作用域），缺字段时回落为不可取消。
#[test]
fn profile_request_parses_optional_request_id() {
    let request =
        RemoteProfileRequest::parse(br#"{"profileId": "p-1", "requestId": " caller-1 "}"#)
            .expect("parse");
    assert_eq!(request.profile_id.as_deref(), Some("p-1"));
    assert_eq!(request.request_id.as_deref(), Some("caller-1"));

    let plain = RemoteProfileRequest::parse(br#"{"profileId": "p-1"}"#).expect("parse");
    assert!(plain.request_id.is_none());
}

/// 取消请求必须带 requestId；缺字段是显式错误，不做模糊匹配。
#[test]
fn cancel_request_requires_request_id() {
    assert_eq!(
        parse_cancel_request(br#"{"requestId": "caller-1"}"#).expect("parse"),
        "caller-1"
    );
    assert!(parse_cancel_request(br#"{}"#).is_err());
    assert!(parse_cancel_request(b"not json").is_err());
}
