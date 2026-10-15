//! 任务索引 syncer 端到端（S3b，docs/specs/rust-u2-tasks-index.md §6.7）：真 WebSocket + 真
//! embedded 实例 + 本地 mock 模型。会话走**纯 V4 命令**创建（不经 `escode-session.createSession`
//! 的索引写），索引行、状态收敛与事件全部由 syncer 的 V4 topic 帧摄入驱动——这正是
//! 「索引随运行时状态保持同步」的验收路径。
mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use support::*;
use tokio::net::TcpListener;

const PROMPT: &str = "请输出魔法词 sync-e2e-9d21";
const REPLY: &str = "sync-e2e-9d21";
const RENAME_TITLE: &str = "手动改名-3c7f";

/// mock provider：先等一小段再回 SSE——保证 syncer 的基线/增量先看到 running 阶段，
/// 终态迁移（terminal transition）可确定性地发生（否则首帧快照可能直接就是 completed）。
async fn mock_provider(calls: Arc<AtomicUsize>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("mock bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let calls = Arc::clone(&calls);
            tokio::spawn(async move {
                let mut buffer = Vec::new();
                let mut chunk = [0_u8; 4096];
                let (head_end, length) = loop {
                    let Ok(read) = tokio::io::AsyncReadExt::read(&mut stream, &mut chunk).await
                    else {
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
                    let Ok(read) = tokio::io::AsyncReadExt::read(&mut stream, &mut chunk).await
                    else {
                        break;
                    };
                    if read == 0 {
                        break;
                    }
                    buffer.extend_from_slice(&chunk[..read]);
                }
                tokio::time::sleep(Duration::from_millis(600)).await;
                calls.fetch_add(1, Ordering::SeqCst);
                let delta = json!({"choices": [{"delta": {"content": REPLY}}]});
                let stop = json!({"choices": [{"delta": {}, "finish_reason": "stop"}],
                    "usage": {"prompt_tokens": 10, "completion_tokens": 4}});
                let body = format!("data: {delta}\n\ndata: {stop}\n\ndata: [DONE]\n\n");
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    format!("http://{addr}/v1")
}

struct Client {
    socket: Socket,
    frames: FrameReader,
    next: i64,
    events: Vec<Value>,
    listen_id: i64,
    workspace: String,
}

impl Client {
    async fn send(&mut self, kind: i64, channel: &str, name: &str, arg: Wire) -> i64 {
        self.next += 1;
        let id = self.next;
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
            .unwrap();
        id
    }

    async fn frame(&mut self) -> (i64, i64, Wire) {
        loop {
            while let Some(body) = self.frames.next_frame().unwrap() {
                let mut reader = Reader::new(&body);
                let Wire::Array(header) = reader.value().unwrap() else {
                    panic!()
                };
                let kind = header[0].as_int().unwrap();
                if kind == session::RESPONSE_INITIALIZE {
                    continue;
                }
                return (kind, header[1].as_int().unwrap(), reader.value().unwrap());
            }
            let message = tokio::time::timeout(Duration::from_secs(60), self.socket.next())
                .await
                .expect("frame in time")
                .unwrap()
                .unwrap();
            if let Message::Binary(bytes) = message {
                self.frames.push(&bytes);
            }
        }
    }

    async fn call(&mut self, channel: &str, name: &str, params: Value) -> Result<Wire, Value> {
        let id = self
            .send(
                session::REQUEST_PROMISE,
                channel,
                name,
                Wire::Array(vec![Wire::Object(params)]),
            )
            .await;
        loop {
            let (kind, response, data) = self.frame().await;
            if kind == session::RESPONSE_EVENT_FIRE && response == self.listen_id {
                if let Wire::Object(event) = data {
                    self.events.push(event);
                }
                continue;
            }
            if response != id {
                continue;
            }
            return if kind == session::RESPONSE_PROMISE_SUCCESS {
                Ok(data)
            } else {
                Err(data.into_object().unwrap_or(Value::Null))
            };
        }
    }

    /// 等到某个 reason 的事件出现（事件与应答的相对顺序不保证）。
    async fn event(&mut self, reason: &str) -> Value {
        loop {
            if let Some(position) = self
                .events
                .iter()
                .position(|event| event["reason"] == reason)
            {
                return self.events.remove(position);
            }
            let (kind, response, data) = self.frame().await;
            if kind == session::RESPONSE_EVENT_FIRE
                && response == self.listen_id
                && let Wire::Object(event) = data
            {
                self.events.push(event);
            }
        }
    }

    /// 轮询索引直到断言成立（syncer 的摄入是异步的）。
    async fn wait_meta(
        &mut self,
        task_id: &str,
        filter: impl Fn(&Value) -> bool,
        what: &str,
    ) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let meta = self
                .call(
                    "escode-task",
                    "getTaskMeta",
                    json!({"workspacePath": self.workspace, "workspaceIdentity": self.workspace, "taskId": task_id}),
                )
                .await
                .ok()
                .and_then(Wire::into_object);
            if let Some(meta) = meta
                && filter(&meta)
            {
                return meta;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for {what}",
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

fn object(wire: Wire) -> Value {
    wire.into_object().expect("object payload")
}

#[tokio::test(flavor = "multi_thread")]
async fn v4_sessions_sync_into_the_task_index_without_the_session_write_path() {
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
    let mut client = Client {
        socket: open(&surface).await,
        frames: FrameReader::new(),
        next: 0,
        events: Vec::new(),
        listen_id: 0,
        workspace: workspace.clone(),
    };
    client.listen_id = client
        .send(
            session::REQUEST_EVENT_LISTEN,
            "escode-task",
            "onDynamicWorkspaceEvent",
            Wire::Object(json!({"workspacePath": workspace, "workspaceIdentity": workspace})),
        )
        .await;

    // 纯 V4 路径：握手 + createSession（带首条输入 → 立即开轮）。这条路径不写 tasks-index，
    // 行的出现、状态收敛、事件全部来自 syncer 的 sessions-index 摄入。
    let hello = object(
        client
            .call("escode-agent", "helloConversationV4", json!({}))
            .await
            .expect("hello"),
    );
    assert_eq!(hello["kind"], "hello");
    client
        .call(
            "escode-agent",
            "initializeConversationV4",
            json!({"kind": "clientHello", "protocolVersion": 3, "clientId": "client-1",
                "appVersion": "test", "capabilities": {}}),
        )
        .await
        .expect("initialize");
    let command = |kind: &str, session: Option<&str>, payload: Value| {
        let mut target = json!({
            "workspacePath": workspace, "workspaceIdentity": workspace,
            "config": model_config.to_string_lossy(),
        });
        target["envelope"] = json!({
            "commandId": format!("cmd-{}", uuid::Uuid::now_v7()),
            "clientId": "client-1",
            "sessionId": session,
            "type": kind,
            "payload": payload,
            "issuedAt": 0,
        });
        target
    };
    let ack = object(
        client
            .call(
                "escode-agent",
                "sendConversationCommandV4",
                command(
                    "createSession",
                    None,
                    json!({"workspaceId": workspace, "firstInput": {"text": PROMPT}}),
                ),
            )
            .await
            .expect("createSession ack"),
    );
    assert_eq!(ack["status"], "accepted", "{ack}");
    let task_id = ack["result"]["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("sessionId in ack: {ack}"))
        .to_owned();

    // syncer 摄入：行出现（became-visible 回源），随后终态收敛 completed + 未读提醒事件。
    let completed = client
        .wait_meta(
            &task_id,
            |meta| meta["status"] == "completed",
            "terminal convergence",
        )
        .await;
    assert_eq!(completed["provider"], "glm");
    let event = client.event("task_status_changed").await;
    assert_eq!(event["taskId"], Value::from(task_id.clone()));
    assert_eq!(event["unreadSignal"], "background_terminal");
    assert_eq!(event["taskMeta"]["status"], "completed");
    assert!(
        calls.load(Ordering::SeqCst) >= 1,
        "the turn reached the model"
    );

    // 回源写入正文搜索索引：listTaskList 按回复正文命中（readSession contentProfile=index）。
    let searched = object(
        client
            .call(
                "escode-task",
                "listTaskList",
                json!({"kind": "timeline", "search": "sync-e2e",
                    "workspaceScopes": [{"workspacePath": workspace, "workspaceIdentity": workspace}]}),
            )
            .await
            .expect("search"),
    );
    assert!(
        searched["items"]
            .as_array()
            .is_some_and(|items| !items.is_empty()),
        "the resync indexed the conversation body: {searched}"
    );

    // 运行时驱动的标题变化（renameSession 命令）→ task_title_changed + 索引标题跟随。
    let rename = object(
        client
            .call(
                "escode-agent",
                "sendConversationCommandV4",
                command(
                    "renameSession",
                    Some(&task_id),
                    json!({"title": RENAME_TITLE}),
                ),
            )
            .await
            .expect("renameSession ack"),
    );
    assert_eq!(rename["status"], "accepted", "{rename}");
    let renamed = client
        .wait_meta(
            &task_id,
            |meta| meta["title"] == RENAME_TITLE,
            "runtime-driven title sync",
        )
        .await;
    // 运行时改名走 `applyAgentPatch`（TS `applyTitleChange`）：只落 title/updatedAt，
    // 不置 `titleOverridden`（那是产品级 `renameTask` 的语义）。
    assert_eq!(renamed["titleOverridden"], false);
    let event = client.event("task_title_changed").await;
    assert_eq!(event["taskId"], Value::from(task_id.clone()));
    assert_eq!(event["taskMeta"]["title"], RENAME_TITLE);

    surface.shutdown().await;
}
