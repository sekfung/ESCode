//! `gen-ui` 端到端（真 WebSocket，`rust-u2-gen-ui.md` §6）。
//!
//! 覆盖：状态四法（含事件去重与模型侧直接改文件引发的通知）与受限文档读取（模型写文件、Host 只读）。
//! `prepareSandbox` 不在本域（Electron Main 的 webview 沙箱，见边界子 spec §3.2）。
mod support;
use support::*;

use std::time::Duration;

/// 一条连接的帧读取器（订阅事件要跨多次读保持 `FrameReader` 的续帧状态）。
struct Conn {
    socket: Socket,
    frames: FrameReader,
    /// 已到达但尚未被 `next_event` 取走的事件帧。
    ///
    /// **必须缓冲**：事件可能先于触发它的那次写调用的应答到达（`setState` 的 publish 与响应
    /// 是两条独立的出站路径，顺序不保证）。边等应答边丢事件会让后续断言空等。
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

    async fn send(&mut self, kind: i64, id: i64, channel: &str, method: &str, arg: &Wire) {
        let mut writer = Writer::new();
        writer.value(&Wire::Array(vec![
            Wire::Int(kind),
            Wire::Int(id),
            Wire::Str(channel.to_owned()),
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

    /// 读下一帧（跳过 `Initialize`）。
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

    /// 调用并等应答（同一 id 的响应）。
    async fn call(&mut self, id: i64, method: &str, params: serde_json::Value) -> Wire {
        self.send(
            session::REQUEST_PROMISE,
            id,
            "gen-ui",
            method,
            &Wire::Array(vec![Wire::Object(params)]),
        )
        .await;
        loop {
            let (kind, response_id, payload) = self.frame().await;
            if kind == session::RESPONSE_EVENT_FIRE {
                self.events.push(payload);
                continue;
            }
            if response_id == id {
                return payload;
            }
        }
    }

    /// 取下一个事件：先清缓冲，再读帧（读到应答帧也不丢，放回缓冲的语义由调用方保证）。
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

    /// 断言在给定时间内**没有**新事件。
    async fn expect_no_event(&mut self, within: Duration) {
        if !self.events.is_empty() {
            panic!("不应有新事件，却收到 {:?}", self.events.remove(0));
        }
        let outcome = tokio::time::timeout(within, self.frame()).await;
        match outcome {
            Err(_) => {}
            Ok((kind, _, payload)) => panic!("不应有新事件，却收到 kind={kind}: {payload:?}"),
        }
    }

    async fn call_ok(&mut self, id: i64, method: &str, params: serde_json::Value) -> Wire {
        self.call(id, method, params).await
    }
}

fn target(path: &str) -> serde_json::Value {
    serde_json::json!({"workspacePath": "/workspace", "sessionId": "sess-1", "path": path})
}

fn scope() -> serde_json::Value {
    serde_json::json!({"workspacePath": "/workspace", "sessionId": "sess-1"})
}

#[tokio::test(flavor = "multi_thread")]
async fn state_round_trip_events_and_restricted_document_read() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let data_base_dir = dir.path().join("data");
    for path in [&home, &data_base_dir] {
        std::fs::create_dir_all(path).unwrap();
    }
    let surface = start_with_data_base_dir(&home, &data_base_dir).await;
    let mut conn = Conn::new(&surface).await;
    let path = "/root/scope/report.html";

    // ── 未写过 → null；写后读回 ─────────────────────────────────────
    let fresh = conn.call_ok(1, "getState", target(path)).await;
    assert!(
        matches!(fresh, Wire::Object(ref value) if value.is_null()),
        "未写过回 null: {fresh:?}"
    );

    conn.call_ok(
        2,
        "setState",
        serde_json::json!({"target": target(path), "state": {"modelContent": {"chart": "bar"}}}),
    )
    .await;
    let read_back = conn.call_ok(3, "getState", target(path)).await;
    assert!(
        matches!(&read_back, Wire::Object(value) if value["modelContent"]["chart"] == serde_json::json!("bar")),
        "读回应含写入内容: {read_back:?}"
    );
    // 缺省键补 null（与 zod 的 `.default(null)` 同形）。
    assert!(
        matches!(&read_back, Wire::Object(value) if value["privateContent"].is_null()),
        "{read_back:?}"
    );

    // ── listState ──────────────────────────────────────────────────
    let entries = conn.call_ok(4, "listState", scope()).await;
    let Wire::Array(items) = entries else {
        panic!("listState 应答应为数组")
    };
    assert_eq!(items.len(), 1, "只有一条记录: {items:?}");
    let Wire::Object(item) = &items[0] else {
        panic!("条目应为对象")
    };
    assert_eq!(item["path"], serde_json::json!(path));

    // ── 订阅事件：新值发、同值不发 ──────────────────────────────────
    conn.send(
        session::REQUEST_EVENT_LISTEN,
        90,
        "gen-ui",
        "onStateChanged",
        &Wire::Undefined,
    )
    .await;

    conn.call_ok(
        5,
        "setState",
        serde_json::json!({"target": target(path), "state": {"modelContent": {"chart": "line"}}}),
    )
    .await;
    let payload = conn.next_event().await;
    let Wire::Object(event) = payload else {
        panic!("事件载荷应为对象")
    };
    assert_eq!(event["target"]["path"], serde_json::json!(path));
    assert_eq!(
        event["state"]["modelContent"]["chart"],
        serde_json::json!("line")
    );

    // 同值再写：不重复发。
    conn.call_ok(
        6,
        "setState",
        serde_json::json!({"target": target(path), "state": {"modelContent": {"chart": "line"}}}),
    )
    .await;
    conn.expect_no_event(Duration::from_millis(300)).await;

    // ── 模型侧直接改文件 → watcher 通知 ────────────────────────────
    let records_dir = std::fs::read_dir(data_base_dir.join(".escode/v2/gen-ui-state"))
        .unwrap()
        .flatten()
        .next()
        .expect("scope 目录已创建")
        .path();
    let record = serde_json::json!({
        "target": target(path),
        "state": {"modelContent": {"chart": "pie"}, "privateContent": null},
        "updatedAt": 9_999_999_999.0_f64,
    });
    // 直接改**已存在**的记录文件（模型改文件走的是同一条路径）。
    let record_file = std::fs::read_dir(&records_dir)
        .unwrap()
        .flatten()
        .find(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.ends_with(".json"))
        })
        .expect("已有记录文件")
        .path();
    std::fs::write(&record_file, serde_json::to_vec(&record).unwrap()).unwrap();
    let payload = tokio::time::timeout(Duration::from_secs(10), conn.next_event())
        .await
        .expect("文件变更应触发通知");
    let Wire::Object(event) = payload else {
        panic!("事件载荷应为对象")
    };
    assert_eq!(
        event["state"]["modelContent"]["chart"],
        serde_json::json!("pie")
    );

    // ── 受限文档读取：模型写的片段可读，输出根外拒绝 ────────────────
    let output_root = data_base_dir.join(".escode/v2/visualizations");
    std::fs::create_dir_all(output_root.join("group")).unwrap();
    let document_path = output_root.join("group/chart-fragment.html");
    std::fs::write(&document_path, "<div id=\"chart\">fragment</div>").unwrap();
    let document = conn
        .call_ok(7, "readDocument", target(&document_path.to_string_lossy()))
        .await;
    let Wire::Object(document) = document else {
        panic!("readDocument 应答应为对象")
    };
    assert_eq!(
        document["html"],
        serde_json::json!("<div id=\"chart\">fragment</div>")
    );
    assert_eq!(
        document["path"],
        serde_json::json!(document_path.to_string_lossy())
    );

    let outside = dir.path().join("outside.html");
    std::fs::write(&outside, "<p>out</p>").unwrap();
    let payload = conn
        .call(8, "readDocument", target(&outside.to_string_lossy()))
        .await;
    let error = payload.into_object().expect("错误载荷");
    assert_eq!(
        error["message"],
        serde_json::json!("Gen UI file is outside the host output directory"),
        "{error}"
    );

    surface.shutdown().await;
}
