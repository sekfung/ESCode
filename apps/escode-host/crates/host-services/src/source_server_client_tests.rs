use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::mpsc;

use super::{
    RESPONSE_EVENT_FIRE, RESPONSE_INITIALIZE, RESPONSE_PROMISE_ERROR, RESPONSE_PROMISE_SUCCESS,
    RemoteChannelClient,
};
use escode_host_core::rpc_serialization::WireValue;
use escode_host_core::rpc_wire::HEADER_SIZE;

fn frame(header: Vec<WireValue>, body: WireValue) -> Vec<u8> {
    let mut bytes = WireValue::Array(header).encode();
    body.encode_into(&mut bytes);
    bytes
}

fn initialize(channels: Option<Vec<&str>>) -> Vec<u8> {
    let body = match channels {
        Some(names) => WireValue::Object(json!({"channels": names})),
        None => WireValue::Undefined,
    };
    frame(vec![WireValue::Int(RESPONSE_INITIALIZE)], body)
}

fn decode_request(bytes: &[u8]) -> (WireValue, WireValue) {
    assert_eq!(bytes[0], 1, "Regular 帧");
    let length = u32::from_be_bytes([bytes[9], bytes[10], bytes[11], bytes[12]]) as usize;
    assert_eq!(bytes.len(), HEADER_SIZE + length, "帧长度与头一致");
    let payload = &bytes[HEADER_SIZE..];
    let (header, used) = WireValue::decode(payload).expect("header");
    let (body, _) = WireValue::decode(&payload[used..]).expect("body");
    (header, body)
}

/// Initialize 后 call 生成与 TS 完全一致的请求字节，并解析 PromiseSuccess。
#[tokio::test]
async fn call_sends_the_typescript_request_and_resolves() {
    let (outbound, mut frames) = mpsc::unbounded_channel();
    let client = Arc::new(RemoteChannelClient::new(outbound));
    client
        .handle_frame(&initialize(Some(vec!["source-server-management"])))
        .expect("initialize");
    assert!(client.has_channel("source-server-management"));
    assert!(!client.has_channel("escode-agent"));

    let pending = tokio::spawn({
        let client = Arc::clone(&client);
        async move { client.call("a", "m", Some(WireValue::Int(1))).await }
    });
    let request = frames.recv().await.expect("request");
    let payload = &request[HEADER_SIZE..];
    assert_eq!(
        payload,
        vec![
            4, 4, // Array(4)
            6, 100, // Promise
            6, 0, // id
            1, 1, b'a', // channel
            1, 1, b'm', // method
            6, 1, // body
        ],
        "请求头与 TS `sendRequest` 逐字节一致"
    );
    let (header, body) = decode_request(&request);
    assert_eq!(
        header,
        WireValue::Array(vec![
            WireValue::Int(100),
            WireValue::Int(0),
            WireValue::String("a".into()),
            WireValue::String("m".into()),
        ])
    );
    assert_eq!(body, WireValue::Int(1));

    client
        .handle_frame(&frame(
            vec![WireValue::Int(RESPONSE_PROMISE_SUCCESS), WireValue::Int(0)],
            WireValue::String("ok".into()),
        ))
        .expect("response");
    assert_eq!(
        pending.await.expect("join").expect("call"),
        WireValue::String("ok".into())
    );
}

/// Initialize 之前调用会等待握手，不丢弃请求。
#[tokio::test]
async fn requests_wait_for_initialize() {
    let (outbound, mut frames) = mpsc::unbounded_channel();
    let client = Arc::new(RemoteChannelClient::new(outbound));
    let pending = tokio::spawn({
        let client = Arc::clone(&client);
        async move { client.call("a", "m", None).await }
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), frames.recv())
            .await
            .is_err(),
        "握手前不应发送请求"
    );
    client.handle_frame(&initialize(None)).expect("initialize");
    let request = frames.recv().await.expect("request after init");
    let (_, body) = decode_request(&request);
    assert_eq!(body, WireValue::Undefined, "缺省参数按 undefined 序列化");
    client
        .handle_frame(&frame(
            vec![WireValue::Int(RESPONSE_PROMISE_SUCCESS), WireValue::Int(0)],
            WireValue::Undefined,
        ))
        .expect("response");
    assert_eq!(
        pending.await.expect("join").expect("call"),
        WireValue::Undefined
    );
    assert!(
        client.channel_names().is_none(),
        "旧 Host 不广播能力时按不可用处理"
    );
}

/// 事件订阅 / 投递 / 取消：EventFire 转发到接收端，dispose 发送 `[103, id]`。
#[tokio::test]
async fn events_are_delivered_and_disposed() {
    let (outbound, mut frames) = mpsc::unbounded_channel();
    let client = Arc::new(RemoteChannelClient::new(outbound));
    client.handle_frame(&initialize(None)).expect("initialize");
    let listen = tokio::spawn({
        let client = Arc::clone(&client);
        async move { client.listen("a", "changed", None).await }
    });
    let request = frames.recv().await.expect("listen request");
    let (header, _) = decode_request(&request);
    assert_eq!(
        header,
        WireValue::Array(vec![
            WireValue::Int(102),
            WireValue::Int(0),
            WireValue::String("a".into()),
            WireValue::String("changed".into()),
        ])
    );
    let (id, mut events) = listen.await.expect("join").expect("listen");
    client
        .handle_frame(&frame(
            vec![
                WireValue::Int(RESPONSE_EVENT_FIRE),
                WireValue::Int(id as i32),
            ],
            WireValue::String("event".into()),
        ))
        .expect("event");
    assert_eq!(
        events.recv().await.expect("event payload"),
        WireValue::String("event".into())
    );
    client.dispose_listener(id);
    let dispose = frames.recv().await.expect("dispose frame");
    let dispose = &dispose[HEADER_SIZE..];
    assert_eq!(
        dispose,
        vec![4, 2, 6, 103, 6, 0, 0],
        "取消订阅 header 只有 [type, id]"
    );
}

/// PromiseError 保留 code/name，供上层映射错误码。
#[tokio::test]
async fn error_response_keeps_code_and_name() {
    let (outbound, mut frames) = mpsc::unbounded_channel();
    let client = Arc::new(RemoteChannelClient::new(outbound));
    client.handle_frame(&initialize(None)).expect("initialize");
    let pending = tokio::spawn({
        let client = Arc::clone(&client);
        async move { client.call("a", "m", None).await }
    });
    frames.recv().await.expect("request");
    client
        .handle_frame(&frame(
            vec![WireValue::Int(RESPONSE_PROMISE_ERROR), WireValue::Int(0)],
            WireValue::Object(json!({
                "message": "denied",
                "name": "SourceServerError",
                "code": "E_AUTH",
            })),
        ))
        .expect("error");
    let error = pending.await.expect("join").expect_err("must fail");
    let error = error
        .downcast::<super::RemoteCallError>()
        .expect("typed error");
    assert_eq!(error.message, "denied");
    assert_eq!(error.name, "SourceServerError");
    assert_eq!(error.data.as_ref().unwrap()["code"], json!("E_AUTH"));
}

/// dispose 拒绝所有挂起请求，Initialize 等待者也失败。
#[tokio::test]
async fn dispose_rejects_pending_calls() {
    let (outbound, mut frames) = mpsc::unbounded_channel();
    let client = Arc::new(RemoteChannelClient::new(outbound));
    client.handle_frame(&initialize(None)).expect("initialize");
    let pending = tokio::spawn({
        let client = Arc::clone(&client);
        async move { client.call("a", "m", None).await }
    });
    frames.recv().await.expect("request");
    client.dispose();
    let error = pending.await.expect("join").expect_err("must fail");
    assert!(error.to_string().contains("disposed"), "{error}");

    let (outbound, _frames) = mpsc::unbounded_channel();
    let waiting = RemoteChannelClient::new(outbound);
    waiting.dispose();
    assert!(waiting.wait_initialized().await.is_err());
}

/// 关闭观察器：已 dispose 的客户端立即返回，未关闭的客户端在 dispose 后唤醒。
#[tokio::test]
async fn wait_closed_observes_disposal() {
    let (outbound, _frames) = mpsc::unbounded_channel();
    let client = Arc::new(RemoteChannelClient::new(outbound));
    client.handle_frame(&initialize(None)).expect("initialize");
    let waiting = tokio::spawn({
        let client = Arc::clone(&client);
        async move { client.wait_closed().await }
    });
    client.dispose();
    tokio::time::timeout(Duration::from_secs(1), waiting)
        .await
        .expect("wait_closed wakes on dispose")
        .expect("join");
}
