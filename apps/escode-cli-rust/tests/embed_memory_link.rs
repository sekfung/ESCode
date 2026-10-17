//! 嵌入实例生命周期（docs/specs/rust-u1-runtime-lib.md §4 验收）：
//! 同进程多实例并发无串扰、关闭后再 spawn、双 spawn 所有权冲突、RuntimeOptions 的
//! env/cwd 生效、收口后残余任务清单为空、进程内控制往返延迟基线。
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use escode_cli_rust::embed::{self, RuntimeOptions};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// 一个嵌入测试的隔离环境：workspace、data dir、home 与实例 env。
struct Fixture {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    data_dir: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("temp dir");
        let workspace = root.path().join("workspace");
        let data_dir = root.path().join("data");
        let home = root.path().join("home");
        for dir in [&workspace, &data_dir, &home] {
            std::fs::create_dir_all(dir).expect("fixture dir");
        }
        Self {
            _root: root,
            workspace,
            data_dir,
            home,
        }
    }

    fn options(&self) -> RuntimeOptions {
        let mut env = BTreeMap::new();
        // test env：模型 IO 轨迹不落盘（model_io::target）。
        env.insert("ESCODE_RUNTIME_ENV".to_owned(), "test".to_owned());
        RuntimeOptions {
            home: self.home.clone(),
            env,
            argv: vec!["escode-cli-rust".to_owned(), "app-server".to_owned()],
            cwd: self.workspace.clone(),
            workspace_identity: None,
            data_dir: Some(self.data_dir.clone()),
            config: Some(self.model_config()),
        }
    }

    /// 同 [`Self::options`]，但模型端点指向本测试的 SSE 桩。
    fn options_with_model(&self, base_url: &str) -> RuntimeOptions {
        let mut options = self.options();
        let path = self._root.path().join("model-write.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&json!({
                "providerId": "fixture",
                "modelId": "fixture-model",
                "reasoningLevel": "none",
                "baseUrl": base_url,
            }))
            .expect("config json"),
        )
        .expect("model config write");
        options.config = Some(path);
        options
    }

    /// 静态模型配置（与 CLI `--config` 同格式）；baseUrl 指向必然拒绝的端口，
    /// 本套测试不发模型请求，配置只为会话创建提供选择。
    fn model_config(&self) -> PathBuf {
        let path = self._root.path().join("model.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&json!({
                "providerId": "fixture",
                "modelId": "fixture-model",
                "reasoningLevel": "none",
                "baseUrl": "http://127.0.0.1:9/v1",
            }))
            .expect("config json"),
        )
        .expect("model config write");
        path
    }
}

/// 一个 NDJSON 帧；超时即失败。
async fn next_frame(io: &mut escode_cli_app_server::EmbeddedIo) -> Value {
    let line = tokio::time::timeout(Duration::from_secs(30), io.line())
        .await
        .expect("frame within timeout")
        .expect("instance output open");
    serde_json::from_slice(&line).expect("NDJSON frame")
}

/// 读帧直到出现指定 id 的应答（其余帧丢弃）。
async fn reply_for(io: &mut escode_cli_app_server::EmbeddedIo, id: &str) -> Value {
    loop {
        let value = next_frame(io).await;
        if value["id"].as_str() == Some(id) {
            return value;
        }
    }
}

/// 读帧直到出现指定 method 的通知。
async fn notice_for(io: &mut escode_cli_app_server::EmbeddedIo, method: &str) -> Value {
    loop {
        let value = next_frame(io).await;
        if value["method"] == method {
            return value;
        }
    }
}

async fn send_json(io: &escode_cli_app_server::EmbeddedIo, value: Value) {
    io.send_line(value.to_string()).await;
}

/// 启动实例并等到协议就绪（startup/storageState 的 ready 帧；checking 先到）。
async fn start_ready(options: RuntimeOptions) -> embed::Embedded {
    let mut embedded = embed::spawn(options);
    loop {
        let frame = notice_for(&mut embedded.io, "startup/storageState").await;
        if frame["params"]["phase"] == "ready" {
            return embedded;
        }
    }
}

/// 结束实例：关写端（EOF）并等待任务收口；返回退出结果。
async fn close(embedded: embed::Embedded) -> anyhow::Result<()> {
    let embed::Embedded {
        io,
        task,
        cancel: _,
        tasks,
    } = embedded;
    drop(io);
    let finished = tokio::time::timeout(Duration::from_secs(30), task)
        .await
        .expect("instance stops after EOF")
        .expect("instance task join");
    let result = finished;
    assert!(
        tasks.running().is_empty(),
        "no residual tasks after shutdown: {:?}",
        tasks.running()
    );
    result
}

async fn list_session_ids(embedded: &mut embed::Embedded) -> Vec<String> {
    send_json(
        &embedded.io,
        json!({"id":"list-1","method":"session/list","params":{}}),
    )
    .await;
    let reply = reply_for(&mut embedded.io, "list-1").await;
    reply["result"]["sessions"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row["sessionId"].as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// 场景 1：实例提供协议服务；关闭写端后按 EOF 收口，残余任务清单为空。
#[tokio::test(flavor = "multi_thread")]
async fn instance_serves_and_ends_cleanly_on_eof() {
    let fixture = Fixture::new();
    let mut embedded = start_ready(fixture.options()).await;
    assert!(list_session_ids(&mut embedded).await.is_empty());
    close(embedded).await.expect("clean instance exit");
}

/// 场景 2：同进程两个 workspace 并发；会话事实互不可见；收口互不影响。
#[tokio::test(flavor = "multi_thread")]
async fn two_instances_run_concurrently_without_crosstalk() {
    let first = Fixture::new();
    let second = Fixture::new();
    let mut embedded_a = start_ready(first.options()).await;
    let mut embedded_b = start_ready(second.options()).await;

    // payload.workspaceId 必须等于实例身份（workspaceIdentity ?? 提交路径）。
    let workspace_id = first.workspace.to_string_lossy().into_owned();
    send_json(
        &embedded_a.io,
        json!({"id":"create-1","method":"v4/command","params":{"commandId":"create-1","clientId":"test","sessionId":null,"type":"createSession","issuedAt":1000,"payload":{"workspaceId":workspace_id,"firstInput":{"text":"run","mode":"yolo"}}}}),
    )
    .await;
    let created = reply_for(&mut embedded_a.io, "create-1").await;
    assert!(
        created["error"].is_null(),
        "createSession on instance A: {created}"
    );

    assert!(
        !list_session_ids(&mut embedded_a).await.is_empty(),
        "instance A sees its own session"
    );
    assert!(
        list_session_ids(&mut embedded_b).await.is_empty(),
        "instance B must not see instance A sessions"
    );

    close(embedded_a).await.expect("instance A clean exit");
    // A 收口后 B 仍在服务。
    assert!(
        list_session_ids(&mut embedded_b).await.is_empty(),
        "instance B still serves after A stopped"
    );
    close(embedded_b).await.expect("instance B clean exit");
}

/// 场景 3：同一 workspace 双实例 → 第二个以 `WorkspaceOwned` 拒绝；
/// 首个关闭后再 spawn 同一 workspace 成功（锁与登记表都随 Guard drop 释放）。
#[tokio::test(flavor = "multi_thread")]
async fn duplicate_ownership_is_rejected_and_respawn_after_close_succeeds() {
    let fixture = Fixture::new();
    let embedded = start_ready(fixture.options()).await;
    let second = embed::spawn(fixture.options());
    let second_result = tokio::time::timeout(Duration::from_secs(30), second.task)
        .await
        .expect("second instance settles")
        .expect("join");
    let error = second_result.expect_err("same workspace must not be owned twice");
    assert!(
        error
            .downcast_ref::<escode_cli_state::WorkspaceOwned>()
            .is_some(),
        "expected WorkspaceOwned, got: {error:#}"
    );

    close(embedded).await.expect("first instance clean exit");

    let mut again = start_ready(fixture.options()).await;
    assert!(list_session_ids(&mut again).await.is_empty());
    close(again).await.expect("respawn clean exit");
}

/// 场景 4：RuntimeOptions 的 env/cwd 生效——相对 data dir 按实例 cwd 解析、
/// data dir 由实例 env 快照给出（进程 env 里没有该变量），databaseId 可证。
#[tokio::test(flavor = "multi_thread")]
async fn runtime_options_env_and_cwd_drive_workspace_resolution() {
    let fixture = Fixture::new();
    let mut options = fixture.options();
    options.data_dir = None;
    options.env.insert(
        "ESCODE_CLI_RUST_DATA_DIR".to_owned(),
        "relative-data".to_owned(),
    );
    let mut embedded = embed::spawn(options);
    let ready = loop {
        let frame = notice_for(&mut embedded.io, "startup/storageState").await;
        if frame["params"]["phase"] == "ready" {
            break frame;
        }
    };
    let expected_dir = escode_cli_host::realpath(fixture.workspace.join("relative-data"))
        .await
        .expect("instance data dir realpath");
    let expected = format!(
        "{:x}",
        Sha256::digest(
            expected_dir
                .join("rust-sessions.sqlite")
                .to_string_lossy()
                .as_bytes()
        )
    );
    assert_eq!(
        ready["params"]["databaseId"], expected,
        "storage path must resolve from RuntimeOptions env/cwd"
    );
    close(embedded).await.expect("clean exit");
}

/// 场景 5：进程内控制往返延迟基线（写回 docs/specs/rust-u1-runtime-lib.md §6）。
#[tokio::test(flavor = "multi_thread")]
async fn in_process_control_round_trip_latency_baseline() {
    let fixture = Fixture::new();
    let mut embedded = start_ready(fixture.options()).await;

    let mut samples = Vec::new();
    for index in 0..50 {
        let id = format!("latency-{index}");
        let started = Instant::now();
        send_json(
            &embedded.io,
            json!({"id":id,"method":"v4/commands/query","params":{"clock":true}}),
        )
        .await;
        let _ = reply_for(&mut embedded.io, &id).await;
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).expect("finite samples"));
    let p50 = samples[samples.len() / 2];
    let p95 = samples[(samples.len() as f64 * 0.95) as usize];
    eprintln!("in-process control RPC: p50={p50:.3} ms p95={p95:.3} ms (n=50)");

    close(embedded).await.expect("clean exit");
}

/// 极简 OpenAI SSE 模型桩：首个主回合返回 `Write(file, content)`，工具结果后返回 stop；
/// 标题请求（`tools` 为空）返回固定标题。每个 fixture 用独立桩，两个 workspace 的
/// 模型请求互不配对。
async fn start_model_stub(
    file_name: &'static str,
    content: &'static str,
) -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("model stub bind");
    let addr = listener.local_addr().expect("model stub addr");
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut buffer = Vec::new();
                let mut chunk = [0u8; 8192];
                let header_end = loop {
                    let read = stream.read(&mut chunk).await.unwrap_or(0);
                    if read == 0 {
                        return;
                    }
                    buffer.extend_from_slice(&chunk[..read]);
                    if let Some(position) = buffer.windows(4).position(|part| part == b"\r\n\r\n") {
                        break position + 4;
                    }
                };
                let headers = String::from_utf8_lossy(&buffer[..header_end]).to_ascii_lowercase();
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("content-length:")
                            .and_then(|value| value.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                while buffer.len() < header_end + length {
                    let read = stream.read(&mut chunk).await.unwrap_or(0);
                    if read == 0 {
                        break;
                    }
                    buffer.extend_from_slice(&chunk[..read]);
                }
                let body = &buffer[header_end..(header_end + length).min(buffer.len())];
                let request: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
                let sse = model_reply(&request, file_name, content);
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    sse.len()
                );
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.write_all(sse.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    (format!("http://{addr}/v1"), handle)
}

fn model_reply(request: &Value, file_name: &str, content: &str) -> String {
    let tools = request["tools"].as_array().map_or(0, Vec::len);
    let after_tool = request["messages"]
        .as_array()
        .is_some_and(|messages| messages.iter().any(|message| message["role"] == "tool"));
    let chunk = if tools == 0 {
        json!({"choices":[{"delta":{"content":"Fixture title"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1}})
    } else if after_tool {
        json!({"choices":[{"delta":{"content":"done"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1}})
    } else {
        let arguments = json!({"file_path": file_name, "content": content}).to_string();
        json!({
            "choices":[{
                "delta":{"tool_calls":[{
                    "index":0,
                    "id":"call-write",
                    "type":"function",
                    "function":{"name":"Write","arguments":arguments},
                }]},
                "finish_reason":"tool_calls",
            }],
            "usage":{"prompt_tokens":1,"completion_tokens":1},
        })
    };
    format!("data: {chunk}\n\ndata: [DONE]\n\n")
}

async fn wait_for_file(path: &std::path::Path) {
    let started = Instant::now();
    loop {
        if tokio::fs::try_exists(path).await.unwrap_or(false) {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "file was not produced: {}",
            path.display()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// 帧泵：代 Host 应答 `session/requestRuntimePreferences`（否则首轮不会启动），
/// 把带 id 的应答转给测试。停止信号触发后 drop 写端，实例按 EOF 收口。
fn start_frame_pump(
    io: escode_cli_app_server::EmbeddedIo,
    outbound: Vec<Value>,
) -> (
    tokio::sync::mpsc::UnboundedReceiver<(String, Value)>,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let (mut reader, writer) = io.split();
    let (replies_tx, replies) = tokio::sync::mpsc::unbounded_channel();
    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        for value in outbound {
            writer.send_line(value.to_string()).await;
        }
        loop {
            tokio::select! {
                _ = &mut stop_rx => break,
                line = reader.line() => {
                    let Some(line) = line else { break };
                    let Ok(value) = serde_json::from_slice::<Value>(&line) else { continue };
                    if value["method"] == "session/requestRuntimePreferences" {
                        let response = json!({"id": value["id"].clone(), "result": {}});
                        writer.send_line(response.to_string()).await;
                        continue;
                    }
                    if let Some(id) = value["id"].as_str() {
                        let _ = replies_tx.send((id.to_owned(), value));
                    }
                }
            }
        }
    });
    (replies, stop_tx, task)
}

async fn wait_reply(
    replies: &mut tokio::sync::mpsc::UnboundedReceiver<(String, Value)>,
    id: &str,
) -> Value {
    loop {
        let (reply_id, value) = tokio::time::timeout(Duration::from_secs(30), replies.recv())
            .await
            .expect("reply within timeout")
            .expect("frame pump alive");
        if reply_id == id {
            return value;
        }
    }
}

/// 场景 6（R3）：两个 workspace 并发各自运行写文件工具，文件只落在各自目录。
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_workspaces_write_tools_stay_in_their_own_directories() {
    let first = Fixture::new();
    let second = Fixture::new();
    let (base_a, stub_a) = start_model_stub("only-a.txt", "written by A").await;
    let (base_b, stub_b) = start_model_stub("only-b.txt", "written by B").await;
    let embedded_a = start_ready(first.options_with_model(&base_a)).await;
    let embedded_b = start_ready(second.options_with_model(&base_b)).await;

    let workspace_a = first.workspace.to_string_lossy().into_owned();
    let workspace_b = second.workspace.to_string_lossy().into_owned();
    let create_a = json!({"id":"create-a","method":"v4/command","params":{"commandId":"create-a","clientId":"test","sessionId":null,"type":"createSession","issuedAt":1000,"payload":{"workspaceId":workspace_a,"firstInput":{"text":"write a file","mode":"yolo"}}}});
    let create_b = json!({"id":"create-b","method":"v4/command","params":{"commandId":"create-b","clientId":"test","sessionId":null,"type":"createSession","issuedAt":1000,"payload":{"workspaceId":workspace_b,"firstInput":{"text":"write a file","mode":"yolo"}}}});
    let embed::Embedded {
        io: io_a,
        task: task_a,
        cancel: _,
        tasks: tasks_a,
    } = embedded_a;
    let embed::Embedded {
        io: io_b,
        task: task_b,
        cancel: _,
        tasks: tasks_b,
    } = embedded_b;
    let (mut replies_a, stop_a, pump_a) = start_frame_pump(io_a, vec![create_a]);
    let (mut replies_b, stop_b, pump_b) = start_frame_pump(io_b, vec![create_b]);
    let created_a = wait_reply(&mut replies_a, "create-a").await;
    let created_b = wait_reply(&mut replies_b, "create-b").await;
    assert!(
        created_a["error"].is_null(),
        "instance A create: {created_a}"
    );
    assert!(
        created_b["error"].is_null(),
        "instance B create: {created_b}"
    );

    let file_a = first.workspace.join("only-a.txt");
    let file_b = second.workspace.join("only-b.txt");
    wait_for_file(&file_a).await;
    wait_for_file(&file_b).await;
    assert_eq!(
        tokio::fs::read_to_string(&file_a).await.expect("A file"),
        "written by A"
    );
    assert_eq!(
        tokio::fs::read_to_string(&file_b).await.expect("B file"),
        "written by B"
    );
    assert!(
        !tokio::fs::try_exists(first.workspace.join("only-b.txt"))
            .await
            .unwrap_or(false),
        "instance A must not write into instance B's workspace"
    );
    assert!(
        !tokio::fs::try_exists(second.workspace.join("only-a.txt"))
            .await
            .unwrap_or(false),
        "instance B must not write into instance A's workspace"
    );

    let _ = stop_a.send(());
    let _ = stop_b.send(());
    pump_a.await.expect("instance A frame pump joins");
    pump_b.await.expect("instance B frame pump joins");
    tokio::time::timeout(Duration::from_secs(30), task_a)
        .await
        .expect("instance A stops after EOF")
        .expect("instance A task join")
        .expect("instance A clean exit");
    tokio::time::timeout(Duration::from_secs(30), task_b)
        .await
        .expect("instance B stops after EOF")
        .expect("instance B task join")
        .expect("instance B clean exit");
    assert!(
        tasks_a.running().is_empty(),
        "instance A has no residual tasks: {:?}",
        tasks_a.running()
    );
    assert!(
        tasks_b.running().is_empty(),
        "instance B has no residual tasks: {:?}",
        tasks_b.running()
    );
    stub_a.abort();
    stub_b.abort();
}
