//! RPC 服务面端到端：真实 WebSocket 上跑完整 `@escode/rpc` 协议。
//!
//! 覆盖：HTTP 契约（`/api/server-info`、一次性 capability、`/ws/host` 鉴权 fail-closed）、
//! `Initialize`、`setting.get/update` 往返、未迁移 channel 的显式 `NotImplemented`。
mod support;
use escode_host_rpc::Channels;
use support::*;

/// 原样发一段 HTTP 请求并读回全部响应文本。
async fn raw_request(addr: std::net::SocketAddr, request: String) -> String {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    String::from_utf8_lossy(&response).into_owned()
}

fn upgrade(addr: std::net::SocketAddr, path: &str, extra: &str) -> String {
    format!(
        "GET {path} HTTP/1.1\r\nhost: {addr}\r\nconnection: Upgrade\r\nupgrade: websocket\r\nsec-websocket-version: 13\r\nsec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\n{extra}\r\n"
    )
}

/// 访问控制（docs/specs/rust-u2-agent-v4.md §3.2，评审 A9）：HTTP 路由要进程密钥；两条 WS 路径都要
/// 一次性 capability；任何带 `Origin` 的请求（浏览器）一律 403；令牌不可预测且一次性。
#[tokio::test(flavor = "multi_thread")]
async fn host_secret_capability_and_origin_gate_the_surface() {
    let dir = tempfile::tempdir().unwrap();
    let surface = start(dir.path()).await;
    let addr = surface.addr;

    let (status, info) = http_request(
        addr,
        "GET",
        "/api/server-info",
        &secret_header(&surface),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(info["capabilities"]["websocketRpc"], true);
    assert_eq!(info["authRequired"], true);
    assert!(
        info["channels"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "setting")
    );

    // 没有/错误的进程密钥：server-info 与 capability 签发都 401。
    for headers in [
        &[][..],
        &[(escode_host_rpc::http::HOST_SECRET_HEADER, "wrong")][..],
    ] {
        let (status, _) = http_request(addr, "GET", "/api/server-info", headers, None).await;
        assert_eq!(status, 401);
        let (status, _) = http_request(
            addr,
            "POST",
            "/api/rpc-host-capability",
            headers,
            Some("{}"),
        )
        .await;
        assert_eq!(status, 401);
    }

    // 浏览器来源：即便带了正确密钥也 403（HTTP 与 WS 升级都一样）。
    let origin = [
        (
            escode_host_rpc::http::HOST_SECRET_HEADER,
            surface.secret.as_str(),
        ),
        ("origin", "http://attacker.example"),
    ];
    let (status, _) = http_request(
        addr,
        "POST",
        "/api/rpc-host-capability",
        &origin,
        Some("{}"),
    )
    .await;
    assert_eq!(status, 403);
    let capability = issue_capability(&surface).await;
    let text = raw_request(
        addr,
        upgrade(
            addr,
            "/ws/host",
            &format!(
                "origin: http://attacker.example\r\n{}: {capability}\r\n",
                escode_host_rpc::http::CAPABILITY_HEADER
            ),
        ),
    )
    .await;
    assert!(text.starts_with("HTTP/1.1 403"), "origin 必须 403: {text}");

    // 两条 WS 路径没有 capability 都 401（旧实现 `/ws` 免鉴权）。
    for path in ["/ws", "/ws/host"] {
        let text = raw_request(addr, upgrade(addr, path, "")).await;
        assert!(
            text.starts_with("HTTP/1.1 401"),
            "{path} 无 capability 必须拒绝: {text}"
        );
    }

    // 令牌：CSPRNG 形态（64 位十六进制），每次不同。
    let first = issue_capability(&surface).await;
    let second = issue_capability(&surface).await;
    assert_ne!(first, second);
    assert_eq!(first.len(), 64);
    assert!(first.chars().all(|c| c.is_ascii_hexdigit()));

    // 带 capability 的连接成功；同一 token 再用必须失败（一次性）。
    let mut socket = connect(addr, "/ws/host", Some(&first)).await;
    call(&mut socket, 1, "setting", "get", &Wire::Undefined).await;
    assert!(
        read_response(&mut socket, 1).await.into_object().is_some(),
        "capability 连接应能调用 setting.get"
    );
    let reused = raw_request(
        addr,
        upgrade(
            addr,
            "/ws/host",
            &format!("{}: {first}\r\n", escode_host_rpc::http::CAPABILITY_HEADER),
        ),
    )
    .await;
    assert!(
        reused.starts_with("HTTP/1.1 401"),
        "一次性 token 复用必须拒绝"
    );
    // `/ws`（replayable）同样用 capability 接入。
    let mut replayable = connect(addr, "/ws", Some(&second)).await;
    call(&mut replayable, 2, "setting", "get", &Wire::Undefined).await;
    assert!(
        read_response(&mut replayable, 2)
            .await
            .into_object()
            .is_some()
    );

    surface.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_round_trip_and_unimplemented_channels_report_gaps() {
    let dir = tempfile::tempdir().unwrap();
    let surface = start(dir.path()).await;
    let mut socket = open(&surface).await;

    // 1. setting.update → 落盘；setting.get 读回。
    call(
        &mut socket,
        7,
        "setting",
        "update",
        &Wire::Array(vec![Wire::Object(serde_json::json!({"locale": "zh-CN"}))]),
    )
    .await;
    assert_eq!(read_response(&mut socket, 7).await, Wire::Undefined);
    call(&mut socket, 8, "setting", "get", &Wire::Undefined).await;
    let settings = read_response(&mut socket, 8)
        .await
        .into_object()
        .expect("object");
    assert_eq!(settings["locale"], "zh-CN");

    // 2. 未迁移 channel：显式 NotImplemented（含已实现清单，Renderer 可诊断）。
    call(
        &mut socket,
        9,
        "workspace",
        "query",
        &Wire::Object(serde_json::json!({})),
    )
    .await;
    let error = read_response(&mut socket, 9)
        .await
        .into_object()
        .expect("error object");
    assert_eq!(error["name"], "NotImplemented");
    let message = error["message"].as_str().unwrap();
    assert!(message.contains("workspace"), "{message}");
    assert!(message.contains("setting"), "{message}");

    // 3. 未知事件：TS `ChannelServer` 不回任何帧（事件没有错误帧）；连接保持可用。
    let mut writer = Writer::new();
    writer.value(&Wire::Array(vec![
        Wire::Int(session::REQUEST_EVENT_LISTEN),
        Wire::Int(10),
        Wire::Str("task".to_owned()),
        Wire::Str("onDidChange".to_owned()),
    ]));
    writer.value(&Wire::Undefined);
    socket
        .send(Message::Binary(
            escode_host_rpc::frame::frame(&writer.into_bytes()).into(),
        ))
        .await
        .unwrap();
    call(&mut socket, 11, "setting", "get", &Wire::Undefined).await;
    let mut frames = FrameReader::new();
    loop {
        let message = tokio::time::timeout(std::time::Duration::from_secs(10), socket.next())
            .await
            .expect("response within timeout")
            .expect("open")
            .expect("frame");
        let Message::Binary(bytes) = message else {
            continue;
        };
        frames.push(&bytes);
        let mut done = false;
        while let Some(body) = frames.next_frame().expect("frame") {
            let mut reader = Reader::new(&body);
            let Wire::Array(header) = reader.value().expect("header") else {
                panic!("header array")
            };
            let id = header.get(1).and_then(Wire::as_int);
            assert_ne!(id, Some(10), "unknown event must not be answered");
            done |= id == Some(11);
        }
        if done {
            break;
        }
    }

    surface.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn non_loopback_bind_is_refused() {
    let channels: Channels = escode_host_rpc::domains::channels(
        std::path::PathBuf::from("/tmp"),
        None,
        escode_host_runtime::Instances::new(escode_host_runtime::Link::Embedded),
        std::sync::Arc::new(|workspace: &escode_host_control::WorkspaceDescriptor| {
            escode_cli_rust::embed::RuntimeOptions {
                home: std::path::PathBuf::from("/tmp"),
                env: std::collections::BTreeMap::new(),
                argv: vec![],
                cwd: std::path::PathBuf::from(&workspace.path),
                workspace_identity: workspace.identity.clone(),
                data_dir: None,
                config: None,
            }
        }),
    );
    let result = escode_host_rpc::serve("0.0.0.0:0".parse().unwrap(), channels, None).await;
    let error = match result {
        Ok(_) => panic!("非回环必须 fail-closed"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("authentication"));
}
