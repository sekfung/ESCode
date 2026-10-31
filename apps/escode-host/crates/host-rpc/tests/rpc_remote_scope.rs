//! 远端 scope capability 的 RPC 面（spec §10.11 验收 2/4）。
//!
//! 不启动真实 transport：ensure 由测试注入，返回一个已 `Initialize` 的 `RemoteChannelClient`，
//! 这样能断言「远端 scope 连接只拿到允许表 + 远端广播过的 channel」以及转发帧的内容。
mod support;

use std::sync::Arc;
use std::time::Duration;

use escode_host_rpc::session::{Channel, Channels, RESPONSE_INITIALIZE, handler_rpc};
use escode_host_rpc::wire::Wire;
use escode_host_rpc::{
    RemoteProfileRequest, RemoteProfileResolveError, RemoteProfileResolver, RemoteScope,
    RemoteScopeEnsurer, ResolvedRemoteProfile,
};
use escode_host_services::source_server_client::{
    REQUEST_PROMISE, RESPONSE_PROMISE_SUCCESS, RemoteChannelClient,
};
use serde_json::json;
use support::*;
use tokio::sync::mpsc;

const REMOTE_CHANNEL: &str = "window-project-changes";

/// 已 `Initialize`（广播 `channels`）的远端客户端 + 出站帧接收端。
fn remote_client(
    channels: &[&str],
) -> (Arc<RemoteChannelClient>, mpsc::UnboundedReceiver<Vec<u8>>) {
    let (outbound, receiver) = mpsc::unbounded_channel();
    let client = Arc::new(RemoteChannelClient::new(outbound));
    let mut payload = escode_host_core::rpc_serialization::WireValue::Array(vec![
        escode_host_core::rpc_serialization::WireValue::Int(RESPONSE_INITIALIZE as i32),
    ])
    .encode();
    escode_host_core::rpc_serialization::WireValue::Object(json!({ "channels": channels }))
        .encode_into(&mut payload);
    client.handle_frame(&payload).expect("initialize frame");
    (client, receiver)
}

fn ensurer_for(client: Arc<RemoteChannelClient>) -> RemoteScopeEnsurer {
    Arc::new(move |_scope: RemoteScope| {
        let client = Arc::clone(&client);
        Box::pin(async move { Ok(client) })
    })
}

/// 本地共享 channel（`setting` 是远端 scope 必须拿不到的代表）+ 允许表里的 `window-controller`。
fn channels(ensurer: RemoteScopeEnsurer) -> Channels {
    let setting = Channel::new(
        "setting",
        vec![(
            "get",
            handler_rpc(|_arg: Wire| {
                Box::pin(async { Ok(Wire::Object(json!({ "local": true }))) })
            }),
        )],
    );
    let window = Channel::new(
        "window-controller",
        vec![(
            "listTaskList",
            handler_rpc(|_arg: Wire| {
                Box::pin(async { Ok(Wire::Object(json!({ "local": true }))) })
            }),
        )],
    );
    Channels::new(vec![setting, window])
        .with_remote_scope(
            ensurer,
            escode_host_rpc::REMOTE_SCOPE_HOST_CHANNELS.to_vec(),
        )
        .with_scoped_set(escode_host_rpc::remote_scope_channel_set())
}

fn profile_resolver(
    resolver: impl Fn(RemoteProfileRequest) -> Result<ResolvedRemoteProfile, RemoteProfileResolveError>
    + Send
    + Sync
    + 'static,
) -> RemoteProfileResolver {
    Arc::new(move |request| {
        let result = resolver(request);
        Box::pin(async move { result })
    })
}

async fn start_with(channels: Channels) -> RpcSurface {
    escode_host_rpc::serve("127.0.0.1:0".parse().unwrap(), channels, None)
        .await
        .expect("rpc surface starts")
}

fn scope_body() -> String {
    json!({
        "scope": {
            "sourceServerId": "33333333-3333-4333-8333-333333333333",
            "profileId": "44444444-4444-4444-8444-444444444444",
            "workspaceIdentity": "/remote/workspace",
        }
    })
    .to_string()
}

/// 读第一帧 Initialize 的 channel 清单。
async fn initialize_channels(socket: &mut Socket) -> Vec<String> {
    let mut frames = FrameReader::new();
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("frame within timeout")
            .expect("socket open")
            .expect("frame ok");
        let bytes: Vec<u8> = match message {
            Message::Binary(bytes) => bytes.to_vec(),
            Message::Text(text) => text.as_bytes().to_vec(),
            _ => continue,
        };
        frames.push(&bytes);
        while let Some(body) = frames.next_frame().expect("frame parses") {
            let mut reader = Reader::new(&body);
            let Wire::Array(header) = reader.value().expect("header") else {
                panic!("response header must be an array")
            };
            if header[0].as_int() != Some(RESPONSE_INITIALIZE) {
                continue;
            }
            let Wire::Object(payload) = reader.value().expect("initialize payload") else {
                panic!("initialize payload must be an object")
            };
            return payload["channels"]
                .as_array()
                .expect("channel list")
                .iter()
                .filter_map(|name| name.as_str().map(str::to_owned))
                .collect();
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn scoped_capability_serves_only_allowed_and_remote_channels() {
    let (client, mut outbound) = remote_client(&[REMOTE_CHANNEL]);
    let surface = start_with(channels(ensurer_for(Arc::clone(&client)))).await;

    let (status, issued) = http_request(
        surface.addr,
        "POST",
        "/api/rpc-host-capability",
        &secret_header(&surface),
        Some(&scope_body()),
    )
    .await;
    assert_eq!(status, 200, "scoped capability issued: {issued}");
    let capability = issued["capability"].as_str().expect("capability string");
    let mut socket = connect(surface.addr, "/ws/host", Some(capability)).await;

    let mut names = initialize_channels(&mut socket).await;
    names.sort();
    assert_eq!(
        names,
        vec!["window-controller".to_owned(), REMOTE_CHANNEL.to_owned()],
        "远端 scope 不得注册本地共享 channel（如 setting）"
    );

    // 本地 setting 在该连接上是显式缺口。
    call(&mut socket, 1, "setting", "get", &Wire::Array(vec![])).await;
    let Wire::Object(error) = read_response(&mut socket, 1).await else {
        panic!("unknown channel returns PromiseError payload");
    };
    assert_eq!(error["name"], json!("NotImplemented"));

    // 远端 channel 的方法转发到 Source Server，响应按 WireValue 转回。
    call(
        &mut socket,
        2,
        REMOTE_CHANNEL,
        "getCapabilities",
        &Wire::Array(vec![]),
    )
    .await;
    // 远端 controller 打开连接时会先订阅 `escode-task` 事件；跳过这些帧，只断言被转发的方法调用。
    let header = loop {
        let frame = tokio::time::timeout(Duration::from_secs(5), outbound.recv())
            .await
            .expect("forwarded frame within timeout")
            .expect("outbound channel open");
        let mut frames = FrameReader::new();
        frames.push(&frame);
        let payload = frames
            .next_frame()
            .expect("frame parses")
            .expect("complete frame");
        // 转发帧 = header + arg 两个序列化值。
        let (value, _) = escode_host_core::rpc_serialization::WireValue::decode(&payload)
            .expect("forwarded request decodes");
        let escode_host_core::rpc_serialization::WireValue::Array(header) = value else {
            panic!("forwarded request header is an array");
        };
        if header[0] != escode_host_core::rpc_serialization::WireValue::Int(REQUEST_PROMISE) {
            continue;
        }
        break header;
    };
    assert_eq!(
        header[2],
        escode_host_core::rpc_serialization::WireValue::String(REMOTE_CHANNEL.to_owned())
    );
    let id = match header[1] {
        escode_host_core::rpc_serialization::WireValue::Int(id) => id,
        ref other => panic!("request id must be an int: {other:?}"),
    };
    let mut response = escode_host_core::rpc_serialization::WireValue::Array(vec![
        escode_host_core::rpc_serialization::WireValue::Int(RESPONSE_PROMISE_SUCCESS),
        escode_host_core::rpc_serialization::WireValue::Int(id),
    ])
    .encode();
    escode_host_core::rpc_serialization::WireValue::Object(json!({ "remote": true }))
        .encode_into(&mut response);
    client.handle_frame(&response).expect("remote response");

    let Wire::Object(result) = read_response(&mut socket, 2).await else {
        panic!("forwarded result is an object");
    };
    assert_eq!(result, json!({ "remote": true }));
}

#[tokio::test(flavor = "multi_thread")]
async fn first_connect_profile_resolution_returns_backfilled_scope() {
    let (client, _outbound) = remote_client(&[]);
    let resolver = profile_resolver(|request| {
        assert_eq!(
            request
                .target
                .as_ref()
                .and_then(|target| target["host"].as_str()),
            Some("example.test")
        );
        Ok(ResolvedRemoteProfile {
            profile_id: "44444444-4444-4444-8444-444444444444".to_owned(),
            source_server_id: "33333333-3333-4333-8333-333333333333".to_owned(),
            target: json!({"kind": "ssh", "host": "example.test", "username": "dev"}),
        })
    });
    let surface =
        start_with(channels(ensurer_for(client)).with_remote_profile_resolver(resolver)).await;

    let body = json!({
        "target": {"kind": "ssh", "host": "example.test", "username": "dev", "password": "secret"}
    })
    .to_string();
    let (status, resolved) = http_request(
        surface.addr,
        "POST",
        "/api/source-server-resolve-profile",
        &secret_header(&surface),
        Some(&body),
    )
    .await;

    assert_eq!(status, 200, "profile resolution failed: {resolved}");
    assert_eq!(
        resolved["profileId"],
        json!("44444444-4444-4444-8444-444444444444")
    );
    assert_eq!(
        resolved["sourceServerId"],
        json!("33333333-3333-4333-8333-333333333333")
    );
    assert_eq!(resolved["target"]["host"], json!("example.test"));
    assert!(!resolved.to_string().contains("secret"));
}

#[tokio::test(flavor = "multi_thread")]
async fn first_connect_identity_change_returns_structured_conflict() {
    let (client, _outbound) = remote_client(&[]);
    let resolver = profile_resolver(|_request| {
        Err(RemoteProfileResolveError {
            message: "identity changed".to_owned(),
            code: Some("source-server-identity-change-pending".to_owned()),
            profile_id: Some("profile-1".to_owned()),
            pending_source_server_id: Some("source-2".to_owned()),
            retryable: Some(false),
        })
    });
    let surface =
        start_with(channels(ensurer_for(client)).with_remote_profile_resolver(resolver)).await;

    let (status, error) = http_request(
        surface.addr,
        "POST",
        "/api/source-server-resolve-profile",
        &secret_header(&surface),
        Some(r#"{"profileId":"profile-1"}"#),
    )
    .await;

    assert_eq!(status, 409);
    assert_eq!(
        error["code"],
        json!("source-server-identity-change-pending")
    );
    assert_eq!(error["profileId"], json!("profile-1"));
    assert_eq!(error["pendingSourceServerId"], json!("source-2"));
    assert_eq!(error["retryable"], json!(false));
}

#[tokio::test(flavor = "multi_thread")]
async fn local_capability_still_serves_every_local_channel() {
    let (client, _outbound) = remote_client(&[]);
    let surface = start_with(channels(ensurer_for(client))).await;
    let mut socket = open(&surface).await;

    let mut names = initialize_channels(&mut socket).await;
    names.sort();
    assert_eq!(
        names,
        vec!["setting".to_owned(), "window-controller".to_owned()]
    );

    call(&mut socket, 1, "setting", "get", &Wire::Array(vec![])).await;
    let Wire::Object(result) = read_response(&mut socket, 1).await else {
        panic!("local setting call returns an object");
    };
    assert_eq!(result, json!({ "local": true }));
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_remote_connection_issues_no_capability() {
    let surface = start_with(channels(Arc::new(|scope: RemoteScope| {
        Box::pin(async move {
            Err::<Arc<RemoteChannelClient>, String>(format!(
                "unknown source server profile: {}",
                scope.profile_id
            ))
        })
    })))
    .await;

    let (status, body) = http_request(
        surface.addr,
        "POST",
        "/api/rpc-host-capability",
        &secret_header(&surface),
        Some(&scope_body()),
    )
    .await;
    assert_eq!(status, 400, "签发失败必须显式报错: {body}");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|error| error.contains("unknown source server profile")),
        "错误原因透传: {body}"
    );
    assert!(
        body.get("capability").is_none(),
        "失败时不得签发任何 capability（含本地权限）: {body}"
    );
}

/// 等待 socket 关闭（跳过关闭前仍在路上的事件帧）。
async fn expect_socket_closed(socket: &mut Socket) {
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return,
                Some(Ok(_)) => continue,
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "remote link loss ends the scope connection");
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_link_close_ends_only_scope_connections() {
    let (client, _outbound) = remote_client(&[REMOTE_CHANNEL]);
    let surface = start_with(channels(ensurer_for(Arc::clone(&client)))).await;

    let (status, issued) = http_request(
        surface.addr,
        "POST",
        "/api/rpc-host-capability",
        &secret_header(&surface),
        Some(&scope_body()),
    )
    .await;
    assert_eq!(status, 200, "scoped capability issued: {issued}");
    let capability = issued["capability"].as_str().expect("capability string");
    let mut remote = connect(surface.addr, "/ws/host", Some(capability)).await;
    let _ = initialize_channels(&mut remote).await;

    let local_capability = issue_capability(&surface).await;
    let mut local = connect(surface.addr, "/ws/host", Some(&local_capability)).await;
    let _ = initialize_channels(&mut local).await;

    // 链路终结（EOF / shutdown）只应关闭绑定该 grant 的连接；本地 scope 不跟随。
    client.dispose();
    expect_socket_closed(&mut remote).await;

    call(&mut local, 10, "setting", "get", &Wire::Array(vec![])).await;
    let Wire::Object(result) = read_response(&mut local, 10).await else {
        panic!("local setting call returns an object");
    };
    assert_eq!(result, json!({ "local": true }));
}
