//! Renderer 传输面端到端（rust-u2-host-skeleton.md §3.4 验收）：真实 WebSocket 往返。
//!
//! 覆盖：capability 握手求交、按到达序登记、请求往返（V4 NDJSON 帧原样）、未覆盖 method
//! 回 `NotImplemented`、多连接共享同一实例并**扇出**、断开一个连接不影响另一个。
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use escode_host_control::WorkspaceDescriptor;
use escode_host_runtime::{Instances, Link};
use escode_host_surface::{Hello, Surface};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

struct Fixture {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    data_dir: PathBuf,
}

impl Fixture {
    fn root(&self) -> &std::path::Path {
        self._root.path()
    }

    fn new() -> Self {
        let root = tempfile::tempdir().expect("temp dir");
        let workspace = root.path().join("workspace");
        let data_dir = root.path().join("data");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(&data_dir).expect("data dir");
        Self {
            _root: root,
            workspace,
            data_dir,
        }
    }

    fn descriptor(&self) -> WorkspaceDescriptor {
        WorkspaceDescriptor {
            path: self.workspace.to_string_lossy().into_owned(),
            identity: Some(self.workspace.to_string_lossy().into_owned()),
            data_dir: Some(self.data_dir.to_string_lossy().into_owned()),
            config: None,
        }
    }
}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn connect(addr: SocketAddr, hello: &Hello) -> Socket {
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
        .await
        .expect("ws connect");
    socket
        .send(Message::Text(serde_json::to_string(hello).unwrap().into()))
        .await
        .expect("hello sent");
    socket
}

async fn next_json(socket: &mut Socket) -> Value {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(30), socket.next())
            .await
            .expect("frame within timeout")
            .expect("socket open")
            .expect("frame ok");
        match message {
            Message::Text(text) => return serde_json::from_str(&text).expect("JSON frame"),
            Message::Close(_) => panic!("socket closed while waiting for a frame"),
            _ => continue,
        }
    }
}

/// 读到指定 id 的应答（丢弃其它帧）。
async fn reply_for(socket: &mut Socket, id: &str) -> Value {
    for _ in 0..50 {
        let frame = next_json(socket).await;
        if frame["id"] == id {
            return frame;
        }
    }
    panic!("no reply for {id}");
}

async fn send_request(socket: &mut Socket, id: &str, method: &str) {
    let line = json!({"id": id, "method": method, "params": {}}).to_string();
    socket
        .send(Message::Text(line.into()))
        .await
        .expect("request sent");
}

/// 测试用的 RuntimeOptions 工厂：与 Host 进程的 `control::runtime_options` 同形，
/// 但 HOME 指向 fixture（不读进程 env）。
fn options_factory(fixture_home: PathBuf) -> escode_host_surface::OptionsFactory {
    std::sync::Arc::new(move |workspace: &WorkspaceDescriptor| {
        let mut env = std::collections::BTreeMap::new();
        if let Some(identity) = &workspace.identity {
            env.insert("ESCODE_WORKSPACE_IDENTITY".to_owned(), identity.clone());
        }
        env.insert("ESCODE_RUNTIME_ENV".to_owned(), "test".to_owned());
        escode_cli_rust::embed::RuntimeOptions {
            home: fixture_home.clone(),
            env,
            argv: vec!["escode-cli-rust".to_owned(), "app-server".to_owned()],
            cwd: PathBuf::from(&workspace.path),
            workspace_identity: workspace.identity.clone(),
            data_dir: workspace.data_dir.clone().map(PathBuf::from),
            config: workspace.config.clone().map(PathBuf::from),
        }
    })
}

fn hello(fixture: &Fixture, window: &str, capabilities: &[&str]) -> Hello {
    Hello {
        window_id: window.to_owned(),
        client_id: "renderer-test".to_owned(),
        workspace: fixture.descriptor(),
        capabilities: capabilities.iter().map(|c| (*c).to_owned()).collect(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn handshake_routes_frames_and_reports_gaps() {
    let fixture = Fixture::new();
    let surface = Surface::start(
        "127.0.0.1:0".parse().expect("bind addr"),
        Instances::new(Link::Embedded),
        options_factory(fixture.root().join("home")),
    )
    .await
    .expect("surface starts");
    let mut socket = connect(
        surface.addr,
        &hello(&fixture, "win-1", &["session/list", "v4/command"]),
    )
    .await;

    // 1. 握手：代次 + 求交后的能力（未声明的 method 不登记）。
    let ready = next_json(&mut socket).await;
    assert_eq!(ready["type"], "ready");
    assert_eq!(ready["generation"], 1);
    let capabilities = ready["capabilities"].as_array().expect("capabilities");
    assert_eq!(capabilities.len(), 2, "只登记求交命中的能力: {ready}");
    // 实例就绪帧会随后到达（与握手同一条出站通道）。
    let _ = next_json(&mut socket).await;

    // 2. 请求往返：覆盖的 method 转给实例，应答原样回传。
    send_request(&mut socket, "list-1", "session/list").await;
    let reply = reply_for(&mut socket, "list-1").await;
    assert_eq!(reply["result"]["sessions"], json!([]));

    // 3. 缺口显式：未协商的 method 回 NotImplemented（同 id，不挂起）。
    send_request(&mut socket, "gap-1", "settings/write").await;
    let gap = reply_for(&mut socket, "gap-1").await;
    assert_eq!(gap["error"]["code"], -32601);
    assert!(
        gap["error"]["message"]
            .as_str()
            .unwrap()
            .contains("settings/write")
    );

    let _ = surface.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn connections_share_one_instance_and_fan_out() {
    let fixture = Fixture::new();
    let surface = Surface::start(
        "127.0.0.1:0".parse().expect("bind addr"),
        Instances::new(Link::Embedded),
        options_factory(fixture.root().join("home")),
    )
    .await
    .expect("surface starts");

    let mut first = connect(surface.addr, &hello(&fixture, "win-1", &["session/list"])).await;
    let first_ready = next_json(&mut first).await;
    assert_eq!(first_ready["generation"], 1);
    let _ = next_json(&mut first).await; // 实例就绪帧

    let mut second = connect(surface.addr, &hello(&fixture, "win-2", &["session/list"])).await;
    let second_ready = next_json(&mut second).await;
    assert_eq!(
        second_ready["generation"], 1,
        "同一 workspace 的第二个窗口复用同一实例代次"
    );

    // 两个连接都收到实例帧（扇出）：第二个窗口的请求会让实例产出应答帧，
    // 第一窗口因共享同一实例也会看到（多窗口共享实例的语义）。
    send_request(&mut second, "list-2", "session/list").await;
    let second_reply = reply_for(&mut second, "list-2").await;
    assert_eq!(second_reply["result"]["sessions"], json!([]));
    let first_sees = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let frame = next_json(&mut first).await;
            if frame["id"] == "list-2" {
                return frame;
            }
        }
    })
    .await
    .expect("shared instance fans out to the first window");
    assert_eq!(first_sees["result"]["sessions"], json!([]));

    // 断开一个连接：另一个仍可服务。
    first.close(None).await.expect("first closes");
    send_request(&mut second, "list-3", "session/list").await;
    let third = reply_for(&mut second, "list-3").await;
    assert_eq!(third["result"]["sessions"], json!([]));

    let _ = surface.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn empty_capabilities_still_allow_events_but_no_requests() {
    let fixture = Fixture::new();
    let surface = Surface::start(
        "127.0.0.1:0".parse().expect("bind addr"),
        Instances::new(Link::Embedded),
        options_factory(fixture.root().join("home")),
    )
    .await
    .expect("surface starts");
    let mut socket = connect(surface.addr, &hello(&fixture, "win-1", &[])).await;
    let ready = next_json(&mut socket).await;
    assert_eq!(ready["capabilities"], json!([]));
    send_request(&mut socket, "any", "session/list").await;
    let gap = reply_for(&mut socket, "any").await;
    assert_eq!(gap["error"]["code"], -32601, "未声明能力即缺口: {gap}");
    let _ = surface.shutdown().await;
}
