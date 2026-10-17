//! 崩溃隔离用例的夹具：独立 workspace 目录与挂起式模型桩（`crash_isolation.rs`）。
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use escode_cli_rust::embed::RuntimeOptions;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

/// 失败保护：正常路径都在通道上等到事件，不会接近这个值。
pub const GUARD: Duration = Duration::from_secs(60);

pub struct Workspace {
    root: tempfile::TempDir,
    workspace: PathBuf,
    data_dir: PathBuf,
    home: PathBuf,
}

impl Workspace {
    pub fn new() -> Self {
        let root = tempfile::tempdir().expect("temp dir");
        let workspace = root.path().join("workspace");
        let data_dir = root.path().join("data");
        let home = root.path().join("home");
        for dir in [&workspace, &data_dir, &home] {
            std::fs::create_dir_all(dir).expect("fixture dir");
        }
        Self {
            root,
            workspace,
            data_dir,
            home,
        }
    }

    pub fn identity(&self) -> String {
        self.workspace.to_string_lossy().into_owned()
    }

    /// 模型端点指向本 workspace 的挂起桩。
    pub fn options(&self, model_url: &str) -> RuntimeOptions {
        let config = self.root.path().join("model.json");
        std::fs::write(
            &config,
            serde_json::to_vec(&json!({
                "providerId": "fixture",
                "modelId": "fixture-model",
                "reasoningLevel": "none",
                "baseUrl": model_url,
            }))
            .expect("config json"),
        )
        .expect("model config write");
        RuntimeOptions {
            home: self.home.clone(),
            env: BTreeMap::from([("ESCODE_RUNTIME_ENV".to_owned(), "test".to_owned())]),
            argv: vec!["escode-cli-rust".to_owned(), "app-server".to_owned()],
            cwd: self.workspace.clone(),
            workspace_identity: None,
            data_dir: Some(self.data_dir.clone()),
            config: Some(config),
        }
    }

    pub fn generate_params(&self, prompt: &str) -> Value {
        json!({
            "workspace": {"workspacePath": self.identity()},
            "prompt": prompt,
            "querySource": "fixture",
        })
    }
}

/// 挂起式 OpenAI SSE 模型桩：每收到一个请求就在 `seen` 上报一次，然后挂住连接，
/// 直到 `release` 收到要回的文本（发送端 drop 则一直挂到连接被对端关闭）。
pub struct HeldModel {
    pub url: String,
    pub seen: mpsc::UnboundedReceiver<()>,
    pub release: Option<oneshot::Sender<String>>,
}

pub async fn held_model() -> HeldModel {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("model stub bind");
    let addr = listener.local_addr().expect("model stub addr");
    let (seen_tx, seen) = mpsc::unbounded_channel();
    let (release, released) = oneshot::channel::<String>();
    let released = Arc::new(tokio::sync::Mutex::new(Some(released)));
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let seen_tx = seen_tx.clone();
            let released = Arc::clone(&released);
            tokio::spawn(async move {
                if !read_request(&mut stream).await {
                    return;
                }
                let _ = seen_tx.send(());
                let Some(released) = released.lock().await.take() else {
                    return;
                };
                let Ok(text) = released.await else {
                    // 发送端 drop：一直挂到对端关闭连接。
                    let mut sink = [0u8; 64];
                    while stream.read(&mut sink).await.unwrap_or(0) > 0 {}
                    return;
                };
                let chunk = json!({
                    "choices": [{"delta": {"content": text}, "finish_reason": "stop"}],
                    "usage": {"prompt_tokens": 1, "completion_tokens": 1},
                });
                let sse = format!("data: {chunk}\n\ndata: [DONE]\n\n");
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
    HeldModel {
        url: format!("http://{addr}/v1"),
        seen,
        release: Some(release),
    }
}

/// 读完一个 HTTP 请求（头 + content-length 的体）；对端提前关闭返回 false。
async fn read_request(stream: &mut tokio::net::TcpStream) -> bool {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        let read = stream.read(&mut chunk).await.unwrap_or(0);
        if read == 0 {
            return false;
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
            return false;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    true
}
