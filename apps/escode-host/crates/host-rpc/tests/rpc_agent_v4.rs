//! `escode-agent` V4 面端到端（docs/specs/rust-u2-agent-v4.md §4-3/§4-4）：真 WebSocket + 真 embedded
//! 实例 + 本地 mock 模型（OpenAI chat-completions SSE）。
//!
//! 一轮真实对话：hello → initialize → `onDynamicConversationFrame` 监听 → `createSession`（带 firstInput）
//! → `subscribeConversationV4` → 帧里出现 mock 模型的回复。该路径此前不可能跑通：Host 不应答
//! runtime 的 `session/requestRuntimePreferences`（评审 A5），且没有事件面与 V4 channel（A10）。
mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use support::*;
use tokio::net::TcpListener;

const REPLY: &str = "MOCK-REPLY-7f3a";

/// 最小 mock provider：任何 POST 都回同一段 SSE（含一条 delta 与 stop）。
async fn mock_provider(calls: Arc<AtomicUsize>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("mock bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let calls = Arc::clone(&calls);
            tokio::spawn(async move {
                let mut buffer = Vec::new();
                let mut chunk = [0_u8; 4096];
                // 读完请求头 + content-length 指定的 body。
                let (head_end, length) = loop {
                    let Ok(read) = stream.read(&mut chunk).await else {
                        return;
                    };
                    if read == 0 {
                        return;
                    }
                    buffer.extend_from_slice(&chunk[..read]);
                    if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buffer[..end]).to_lowercase();
                        let length = head
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length:"))
                            .and_then(|value| value.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        break (end + 4, length);
                    }
                };
                while buffer.len() < head_end + length {
                    let Ok(read) = stream.read(&mut chunk).await else {
                        return;
                    };
                    if read == 0 {
                        break;
                    }
                    buffer.extend_from_slice(&chunk[..read]);
                }
                calls.fetch_add(1, Ordering::SeqCst);
                let delta = json!({"choices": [{"delta": {"content": REPLY}}]});
                let stop = json!({"choices": [{"delta": {}, "finish_reason": "stop"}],
                    "usage": {"prompt_tokens": 10, "completion_tokens": 4}});
                let body = format!("data: {delta}\n\ndata: {stop}\n\ndata: [DONE]\n\n");
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    format!("http://{addr}/v1")
}

struct Session {
    socket: Socket,
    frames: FrameReader,
    next_id: i64,
    /// 已收到但尚未被消费的 (类型, id, 数据)。
    backlog: Vec<(i64, i64, Wire)>,
}

impl Session {
    async fn send(&mut self, kind: i64, channel: &str, name: &str, arg: Wire) -> i64 {
        self.next_id += 1;
        let id = self.next_id;
        let mut writer = Writer::new();
        writer.value(&Wire::Array(vec![
            Wire::Int(kind),
            Wire::Int(id),
            Wire::Str(channel.to_owned()),
            Wire::Str(name.to_owned()),
        ]));
        writer.value(&arg);
        self.socket
            .send(Message::Binary(
                escode_host_rpc::frame::frame(&writer.into_bytes()).into(),
            ))
            .await
            .expect("send");
        id
    }

    async fn next(&mut self) -> (i64, i64, Wire) {
        loop {
            while let Some(body) = self.frames.next_frame().expect("frame") {
                let mut reader = Reader::new(&body);
                let Wire::Array(header) = reader.value().expect("header") else {
                    panic!("header array")
                };
                let kind = header[0].as_int().expect("kind");
                if kind == session::RESPONSE_INITIALIZE {
                    continue;
                }
                return (
                    kind,
                    header[1].as_int().expect("id"),
                    reader.value().expect("payload"),
                );
            }
            let message = tokio::time::timeout(Duration::from_secs(60), self.socket.next())
                .await
                .expect("frame within timeout")
                .expect("open")
                .expect("frame ok");
            if let Message::Binary(bytes) = message {
                self.frames.push(&bytes);
            }
        }
    }

    /// 调用一个方法并等它的应答（期间到达的事件进 backlog）。
    async fn call(&mut self, name: &str, arg: Value) -> Result<Value, Value> {
        let id = self
            .send(
                session::REQUEST_PROMISE,
                "escode-agent",
                name,
                Wire::Array(vec![Wire::Object(arg)]),
            )
            .await;
        loop {
            let (kind, response_id, data) = self.next().await;
            if response_id != id {
                self.backlog.push((kind, response_id, data));
                continue;
            }
            let value = match data {
                Wire::Object(value) => value,
                Wire::Undefined => Value::Null,
                other => panic!("unexpected payload {other:?}"),
            };
            return if kind == session::RESPONSE_PROMISE_SUCCESS {
                Ok(value)
            } else {
                Err(value)
            };
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_real_conversation_streams_frames_through_the_v4_scope() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let data_base_dir = dir.path().join("data-base");
    let workspace_dir = dir.path().join("workspace");
    for path in [&home, &data_base_dir, &workspace_dir] {
        std::fs::create_dir_all(path).unwrap();
    }
    support::prepare_index(&data_base_dir);
    let workspace = workspace_dir.to_string_lossy().into_owned();
    let calls = Arc::new(AtomicUsize::new(0));
    let base_url = mock_provider(Arc::clone(&calls)).await;
    let model_config = dir.path().join("model.json");
    std::fs::write(
        &model_config,
        serde_json::to_vec(&json!({
            "apiType": "openai-chat-completions",
            "providerId": "fixture",
            "modelId": "fixture-model",
            "reasoningLevel": "none",
            "contextWindow": 128000,
            "baseUrl": base_url,
        }))
        .unwrap(),
    )
    .unwrap();
    let surface = start_with_data_base_dir(&home, &data_base_dir).await;
    let mut session = Session {
        socket: open(&surface).await,
        frames: FrameReader::new(),
        next_id: 0,
        backlog: Vec::new(),
    };
    let target = json!({
        "workspacePath": workspace,
        "workspaceIdentity": workspace,
        "config": model_config.to_string_lossy(),
    });

    // 1. 未握手：订阅被拒（TS `assertReady`）。
    let error = session
        .call("subscribeSessionsIndexV4", target.clone())
        .await
        .expect_err("handshake required");
    assert_eq!(
        error["message"], "fault.connection.handshakeRequired",
        "{error}"
    );

    // 2. hello → initialize（clientMode 来自连接路径 /ws/host）。
    let hello = session
        .call("helloConversationV4", json!(null))
        .await
        .expect("hello");
    assert_eq!(hello["kind"], "hello");
    assert_eq!(hello["protocolVersion"], 3);
    assert_eq!(hello["clientMode"], "desktop-continuous");
    assert_eq!(hello["deliveryProfile"], "continuous");
    assert!(
        hello["connectionId"]
            .as_str()
            .unwrap()
            .starts_with("host-rpc-")
    );
    let client_hello = json!({"kind": "clientHello", "protocolVersion": 3, "clientId": "client-1",
        "appVersion": "test", "capabilities": {"workflowRunDeltas": true}});
    session
        .call("initializeConversationV4", client_hello)
        .await
        .expect("initialize");

    // 3. 冷 workspace 上的被动订阅（existing-only）进入 dormant：结构化 code 透传。
    let cold_dir = dir.path().join("cold");
    std::fs::create_dir_all(&cold_dir).unwrap();
    let cold = cold_dir.to_string_lossy().into_owned();
    let error = session
        .call(
            "subscribeSessionsIndexV4",
            json!({"workspacePath": cold, "workspaceIdentity": cold, "runtimePolicy": "existing-only"}),
        )
        .await
        .expect_err("dormant");
    assert_eq!(error["code"], "ESCODE_AGENT_RUNTIME_UNAVAILABLE", "{error}");

    // 4. 先挂帧监听（Renderer 同序：workspace supervisor 先 listen 再 subscribe）。
    let listen = session
        .send(
            session::REQUEST_EVENT_LISTEN,
            "escode-agent",
            "onDynamicConversationFrame",
            Wire::Object(json!({"workspacePath": workspace, "workspaceIdentity": workspace})),
        )
        .await;

    // 5. clientId 与握手绑定不一致的命令被拒（不静默覆盖）。
    let envelope = |client: &str| {
        json!({
            "commandId": uuid_like(),
            "clientId": client,
            "sessionId": null,
            "type": "createSession",
            "payload": {"workspaceId": workspace, "firstInput": {"text": "say the magic word"}},
            "issuedAt": 0,
        })
    };
    let mut forged = target.clone();
    forged["envelope"] = envelope("someone-else");
    let error = session
        .call("sendConversationCommandV4", forged)
        .await
        .expect_err("client mismatch");
    assert_eq!(error["message"], "fault.command.clientMismatch");

    // 6. 建会话（带首条输入 → 立即开轮）。
    let turn_started = std::time::Instant::now();
    let mut command = target.clone();
    command["envelope"] = envelope("client-1");
    let ack = session
        .call("sendConversationCommandV4", command)
        .await
        .expect("createSession ack");
    assert_eq!(ack["status"], "accepted", "{ack}");
    let session_id = ack["result"]["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("ack carries the new sessionId: {ack}"))
        .to_owned();

    // 7. 订阅会话 topic：ACK 带 subscriptionId 与 Host 计时（openTiming.version = 1）。
    let mut subscribe = target.clone();
    subscribe["sessionId"] = Value::from(session_id.clone());
    let subscribed = session
        .call("subscribeConversationV4", subscribe)
        .await
        .expect("subscribe");
    let subscription_id = subscribed["ack"]["subscriptionId"]
        .as_str()
        .expect("subscriptionId")
        .to_owned();
    assert_eq!(
        subscribed["ack"]["openTiming"]["version"], 1,
        "{subscribed}"
    );
    assert_eq!(subscribed["ack"]["openTiming"]["cliProcessState"], "reused");

    // 8. 帧：只投递本连接拥有的订阅；一直收到 mock 模型回复出现为止（回合真实跑完）。
    let mut seen_reply = false;
    let mut frames = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let mut pending: Vec<(i64, i64, Wire)> = std::mem::take(&mut session.backlog);
    while !seen_reply {
        assert!(
            tokio::time::Instant::now() < deadline,
            "no frame carried the model reply (calls={})",
            calls.load(Ordering::SeqCst)
        );
        let (kind, id, data) = if pending.is_empty() {
            session.next().await
        } else {
            pending.remove(0)
        };
        if kind != session::RESPONSE_EVENT_FIRE || id != listen {
            continue;
        }
        let Wire::Object(frame) = data else {
            panic!("frame object")
        };
        assert_eq!(
            frame["subscriptionId"],
            Value::from(subscription_id.clone()),
            "foreign frame {frame}"
        );
        assert_eq!(
            frame["topic"],
            Value::from(format!("conversation/{session_id}"))
        );
        frames += 1;
        seen_reply = frame.to_string().contains(REPLY);
    }
    assert!(frames >= 1);
    assert!(
        calls.load(Ordering::SeqCst) >= 1,
        "the turn reached the model"
    );
    // 反向请求（runtime 偏好）若无人应答，runtime 要等满自身超时才继续（实测约 30 s）；
    // Host 应答后回合在 mock 模型下亚秒级完成。
    let elapsed = turn_started.elapsed();
    assert!(
        elapsed < Duration::from_secs(10),
        "turn stalled for {elapsed:?} (unanswered runtime→Host requests?)"
    );

    // 9. 订阅归属：不是本连接的 subscriptionId 不能 resync；unsubscribe 后再 resync 同样 notOwned。
    let mut foreign = target.clone();
    foreign["subscriptionId"] = Value::from("sub_not_mine");
    let error = session
        .call("resyncConversationV4", foreign)
        .await
        .expect_err("not owned");
    assert_eq!(error["message"], "fault.subscription.notOwned");
    let mut owned = target.clone();
    owned["subscriptionId"] = Value::from(subscription_id.clone());
    session
        .call("unsubscribeConversationV4", owned.clone())
        .await
        .expect("unsubscribe");
    let error = session
        .call("resyncConversationV4", owned)
        .await
        .expect_err("forgotten");
    assert_eq!(error["message"], "fault.subscription.notOwned");

    surface.shutdown().await;
}

fn uuid_like() -> String {
    format!(
        "cmd-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}
