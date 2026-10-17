//! `serial` 端到端（真 WebSocket，`rust-u2-serial.md` §7）。
//!
//! 覆盖 S1 + S2：`list` 回数组、`setWatching(true)` 立即触发 `onPorts`；
//! 会话侧用**无硬件**可验证的路径：非法配置 / 不存在的设备报 `SerialError` 带 `code`、
//! 未知路径回 closed 快照、`listSessions` 为空、失败时 `onStatus` 落 `opening → error`。
mod support;
use support::*;

use std::time::Duration;

/// 一条连接的帧读取器（订阅事件要跨多次读保持 `FrameReader` 的续帧状态）。
struct Conn {
    socket: Socket,
    frames: FrameReader,
    events: Vec<Wire>,
}

impl Conn {
    async fn new(surface: &RpcSurface) -> Self {
        Self {
            socket: open(surface).await,
            frames: FrameReader::new(),
            events: Vec::new(),
        }
    }

    async fn send(&mut self, kind: i64, id: i64, method: &str, arg: &Wire) {
        let mut writer = Writer::new();
        writer.value(&Wire::Array(vec![
            Wire::Int(kind),
            Wire::Int(id),
            Wire::Str("serial".to_owned()),
            Wire::Str(method.to_owned()),
        ]));
        writer.value(arg);
        self.socket
            .send(Message::Binary(
                escode_host_rpc::frame::frame(&writer.into_bytes()).into(),
            ))
            .await
            .expect("frame sent");
    }

    async fn frame(&mut self) -> (i64, i64, Wire) {
        loop {
            while let Some(body) = self.frames.next_frame().expect("frame parses") {
                let mut reader = Reader::new(&body);
                let Wire::Array(header) = reader.value().expect("header") else {
                    panic!("response header must be an array")
                };
                let kind = header[0].as_int().expect("kind");
                if kind == session::RESPONSE_INITIALIZE {
                    continue;
                }
                return (
                    kind,
                    header[1].as_int().unwrap_or_default(),
                    reader.value().expect("payload"),
                );
            }
            let message = tokio::time::timeout(Duration::from_secs(10), self.socket.next())
                .await
                .expect("frame within timeout")
                .expect("socket open")
                .expect("frame ok");
            match message {
                Message::Binary(bytes) => self.frames.push(&bytes),
                Message::Text(text) => self.frames.push(text.as_bytes()),
                _ => continue,
            }
        }
    }

    async fn call(&mut self, id: i64, method: &str, params: serde_json::Value) -> Wire {
        self.send(
            session::REQUEST_PROMISE,
            id,
            method,
            &Wire::Array(vec![Wire::Object(params)]),
        )
        .await;
        loop {
            let (kind, response_id, payload) = self.frame().await;
            if kind == session::RESPONSE_EVENT_FIRE {
                // 事件可能先于应答到达，必须缓冲（同 gen-ui 的端到端）。
                self.events.push(payload);
                continue;
            }
            if response_id == id {
                return payload;
            }
        }
    }

    /// 订阅 `onStatus`：只发 EventListen；随后到达的事件帧会被 `call()` 缓冲进 `events`。
    async fn listen_status(&mut self) {
        self.send(
            session::REQUEST_EVENT_LISTEN,
            91,
            "onStatus",
            &Wire::Undefined,
        )
        .await;
    }

    async fn next_event(&mut self) -> Wire {
        if !self.events.is_empty() {
            return self.events.remove(0);
        }
        loop {
            let (kind, _, payload) = self.frame().await;
            if kind == session::RESPONSE_EVENT_FIRE {
                return payload;
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn list_and_watching_broadcast_the_current_port_set() {
    let dir = tempfile::tempdir().unwrap();
    let surface = start(dir.path()).await;
    let mut conn = Conn::new(&surface).await;

    // `list` 回数组（本机无串口时为空）。
    let ports = conn.call(1, "list", serde_json::json!({})).await;
    let Wire::Array(items) = ports else {
        panic!("list 应答应为数组")
    };
    for item in items {
        assert!(
            item.clone()
                .into_object()
                .is_some_and(|port| port.is_object()),
            "端口项应为对象: {item:?}"
        );
    }

    // 订阅后开启 watching：立即收到一次当前端口集（即使为空）。
    conn.send(
        session::REQUEST_EVENT_LISTEN,
        90,
        "onPorts",
        &Wire::Undefined,
    )
    .await;
    conn.call(2, "setWatching", serde_json::json!({"watching": true}))
        .await;
    let first = conn.next_event().await;
    let Wire::Object(first) = first else {
        panic!("onPorts 事件载荷应为数组包装对象")
    };
    assert!(
        first.as_array().is_some(),
        "onPorts 载荷应为端口数组: {first:?}"
    );

    // 关掉 watching；再开一次会产生一次新广播（`lastPortsKey` 被清空）。
    conn.call(3, "setWatching", serde_json::json!({"watching": false}))
        .await;
    conn.call(4, "setWatching", serde_json::json!({"watching": true}))
        .await;
    let again = tokio::time::timeout(Duration::from_secs(5), conn.next_event())
        .await
        .expect("重新开启 watching 应立即再广播一次");
    assert!(matches!(again, Wire::Object(_)));

    // ── S2 会话：无硬件下的错误路径与状态事件 ──────────────────────
    conn.listen_status().await;
    let error = conn
        .call(
            10,
            "open",
            serde_json::json!({"path": "/dev/esc-no-such-port", "config": invalid_config()}),
        )
        .await
        .into_object()
        .expect("错误载荷");
    assert_eq!(error["name"], serde_json::json!("SerialError"));
    assert_eq!(error["code"], serde_json::json!("invalidConfig"), "{error}");

    // 合法配置但设备不存在：错误码来自消息映射（notFound）。
    let error = conn
        .call(
            11,
            "open",
            serde_json::json!({"path": "/dev/esc-no-such-port", "config": valid_config()}),
        )
        .await
        .into_object()
        .expect("错误载荷");
    assert_eq!(error["name"], serde_json::json!("SerialError"));
    assert_eq!(error["code"], serde_json::json!("notFound"), "{error}");

    // 失败也会广播 opening → error。
    let Wire::Object(first) = conn.next_event().await else {
        panic!("状态事件应为对象")
    };
    assert_eq!(first["state"], serde_json::json!("opening"));
    let Wire::Object(second) = conn.next_event().await else {
        panic!("状态事件应为对象")
    };
    assert_eq!(second["state"], serde_json::json!("error"));
    assert_eq!(second["error"]["code"], serde_json::json!("notFound"));

    // 未知路径快照：closed 空快照；没有活动会话。
    let snapshot = conn
        .call(12, "getSnapshot", serde_json::json!({"path": "/dev/never"}))
        .await;
    let Wire::Object(snapshot) = snapshot else {
        panic!("快照应为对象")
    };
    assert_eq!(snapshot["status"]["state"], serde_json::json!("closed"));
    assert_eq!(snapshot["chunks"], serde_json::json!([]));
    // `error` 也算活动会话（TS `isActive = state !== "closed"`）。
    let sessions = conn.call(13, "listSessions", serde_json::json!({})).await;
    let Wire::Array(items) = &sessions else {
        panic!("listSessions 应答应为数组")
    };
    assert_eq!(items.len(), 1, "{sessions:?}");
    assert!(
        matches!(&items[0], Wire::Object(item) if item["status"]["state"] == serde_json::json!("error")),
        "{sessions:?}"
    );
    // 关掉它之后列表清空；对未知路径 `close` 仍是空操作。
    conn.call(
        14,
        "close",
        serde_json::json!({"path": "/dev/esc-no-such-port"}),
    )
    .await;
    let sessions = conn.call(15, "listSessions", serde_json::json!({})).await;
    assert!(
        matches!(&sessions, Wire::Array(items) if items.is_empty()),
        "关闭后不应还有活动会话: {sessions:?}"
    );
    conn.call(16, "close", serde_json::json!({"path": "/dev/never"}))
        .await;

    surface.shutdown().await;
}

fn valid_config() -> serde_json::Value {
    serde_json::json!({
        "baudRate": 115200, "dataBits": 8, "parity": "none",
        "stopBits": 1, "rtscts": false, "autoReconnect": false
    })
}

fn invalid_config() -> serde_json::Value {
    serde_json::json!({
        "baudRate": 115200, "dataBits": 9, "parity": "none",
        "stopBits": 1, "rtscts": false, "autoReconnect": false
    })
}
