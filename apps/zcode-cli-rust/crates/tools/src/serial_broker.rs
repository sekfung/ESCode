//! 串口 broker（docs/specs/serial-agent-tools.md），对齐 TS `serial-broker.ts` 与 `serial-control-broker.ts`：
//! 监听私有 socket，校验 token 与 runtime scope，把 serial MCP server 的请求转成 Host 的 `interaction/serial*`。
//! 会话归属以 hub 记住的该会话 MCP 调用 `_meta` 为准；参数的严格校验由 Host 完成，这里只校验形状。
use crate::contract::{Event, EventSink};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, OnceLock},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub(super) const SOCKET_ENV: &str = "ZCODE_SERIAL_BROKER_SOCKET";
pub(super) const TOKEN_ENV: &str = "ZCODE_SERIAL_BROKER_TOKEN";
const CANCEL_METHOD: &str = "interaction/serialCancel";
const MAX_REQUEST_BYTES: usize = 1024 * 1024;

/// broker 失败：code 原样作为工具错误码返回给 MCP server（如 busy、notOpen、unavailable）。
#[derive(Debug, PartialEq)]
struct BrokerError {
    code: String,
    message: String,
}
impl BrokerError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.into(), message: message.into() }
    }
}

fn method(op: &str) -> Option<&'static str> {
    Some(match op {
        "list" => "interaction/serialList",
        "open" => "interaction/serialOpen",
        "write" => "interaction/serialWrite",
        "read" => "interaction/serialRead",
        "waitFor" => "interaction/serialWaitFor",
        "close" => "interaction/serialClose",
        "setSignals" => "interaction/serialSetSignals",
        _ => return None,
    })
}

pub(super) struct SerialBroker {
    pub socket: String,
    pub token: String,
    host: Arc<OnceLock<EventSink>>,
    /// 会话的请求上下文（该会话 MCP 调用的 `_meta`）：Host 请求的 workspace 身份取自这里。
    sessions: Mutex<BTreeMap<String, Value>>,
}

impl SerialBroker {
    pub fn start(host: Arc<OnceLock<EventSink>>) -> Option<Arc<Self>> {
        let id = uuid::Uuid::new_v4();
        let socket = if cfg!(windows) {
            format!(r"\\.\pipe\zcode-serial-{id}")
        } else {
            std::env::temp_dir().join(format!("zsr-{id}.sock")).to_string_lossy().into_owned()
        };
        let token = super::mcp_oauth_flow::hex(&zcode_cli_host::credential_cipher::random_bytes(32));
        let broker = Arc::new(Self { socket, token, host, sessions: Mutex::default() });
        let serving = broker.clone();
        super::browser_broker_listen::listen_socket(
            &broker.socket,
            Arc::new(move |stream| Box::pin(serving.clone().serve(stream))),
        )
        .ok()?;
        Some(broker)
    }
    pub fn remember(&self, session: &str, meta: &Value) {
        if meta.is_object() {
            self.sessions.lock().unwrap().insert(session.into(), meta.clone());
        }
    }
    pub fn env(&self) -> Value {
        json!({SOCKET_ENV: self.socket, TOKEN_ENV: self.token})
    }

    /// 单个连接：读一行请求，执行并回写一行响应；执行期间对端断开即放弃并通知 Host 取消等待。
    pub async fn serve<S: AsyncRead + AsyncWrite + Unpin + Send>(self: Arc<Self>, mut stream: S) {
        let mut id = uuid::Uuid::new_v4().to_string();
        let outcome: Result<Value, BrokerError> = async {
            let line = read_line(&mut stream).await?;
            let payload: Value =
                serde_json::from_str(&line).map_err(|e| BrokerError::new("invalidInput", e.to_string()))?;
            if let Some(value) = payload["id"].as_str().filter(|v| uuid::Uuid::parse_str(v).is_ok()) {
                id = value.to_owned();
            }
            self.authorize(&payload)?;
            let (reader, _writer) = tokio::io::split(&mut stream);
            tokio::select! {
                result = self.execute(&payload) => result,
                _ = closed(reader) => Err(BrokerError::new("io", "Serial broker peer disconnected")),
            }
        }
        .await;
        let response = match outcome {
            Ok(result) => json!({"id": id, "ok": true, "result": result}),
            Err(error) => json!({"id": id, "ok": false, "error": {"code": error.code, "message": error.message}}),
        };
        let _ = stream.write_all(format!("{response}\n").as_bytes()).await;
        let _ = stream.shutdown().await;
    }

    fn authorize(&self, request: &Value) -> Result<(), BrokerError> {
        let token = request["token"].as_str().unwrap_or_default();
        let (actual, expected) = (token.as_bytes(), self.token.as_bytes());
        let equal = actual.len() == expected.len()
            && actual.iter().zip(expected).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0;
        if !equal {
            return Err(BrokerError::new("unauthorized", "Serial broker request is not authorized"));
        }
        // 子 agent 不得操作硬件：避免多个子 agent 同时往同一串口写入（TS authorize 同）。
        if request["runtimeScope"] == "subagent" {
            return Err(BrokerError::new("unavailable", "Serial port tools are not available in subagents"));
        }
        Ok(())
    }

    /// TS `createProtocolSerialControlPort`：按会话补齐 workspace 身份，生成 Host 请求参数。
    fn params(&self, request: &Value) -> Result<(&'static str, Value), BrokerError> {
        let op = request["op"].as_str().unwrap_or_default();
        let method = method(op).ok_or_else(|| BrokerError::new("invalidInput", format!("Unknown serial op: {op}")))?;
        if !request["args"].is_object() {
            return Err(BrokerError::new("invalidInput", "Serial broker request is missing args"));
        }
        let session = request["sessionId"].as_str().filter(|s| !s.trim().is_empty());
        let session = session.ok_or_else(|| BrokerError::new("invalidInput", "Serial request is missing sessionId"))?;
        let sessions = self.sessions.lock().unwrap();
        let meta = sessions
            .get(session)
            .ok_or_else(|| BrokerError::new("unavailable", format!("Session not found: {session}")))?;
        let mut params = json!({
            "requestId": uuid::Uuid::new_v4().to_string(),
            "sessionId": session,
            "workspaceKey": meta["workspace_key"],
            "workspacePath": meta["workspace_path"],
            "args": request["args"],
        });
        if let Some(turn) = request["turnId"].as_str().or_else(|| meta["turn_id"].as_str()) {
            params["turnId"] = turn.into();
        }
        if let Some(identity) = meta["workspace_identity"].as_str() {
            params["workspaceIdentity"] = identity.into();
        }
        if let Some(remote) = meta["remote_session_id"].as_str() {
            params["remoteSessionId"] = remote.into();
        }
        Ok((method, params))
    }

    async fn execute(&self, request: &Value) -> Result<Value, BrokerError> {
        let (method, params) = self.params(request)?;
        let cancel = (request["op"] == "waitFor").then(|| CancelOnDrop {
            host: self.host.get().cloned(),
            params: Some(json!({"sessionId": params["sessionId"], "targetRequestId": params["requestId"]})),
        });
        let result = self.host_request(method, params).await;
        if let Some(cancel) = cancel {
            std::mem::forget(cancel.disarm());
        }
        result
    }

    async fn host_request(&self, method: &str, params: Value) -> Result<Value, BrokerError> {
        let host = self
            .host
            .get()
            .ok_or_else(|| BrokerError::new("unavailable", "Serial host is unavailable"))?;
        let (reply, receive) = tokio::sync::oneshot::channel();
        host.send(Event::HostRequest { method: method.into(), params, reply })
            .await
            .map_err(|e| BrokerError::new("io", e.to_string()))?;
        match receive.await.map_err(|e| BrokerError::new("io", e.to_string()))? {
            Ok(text) => serde_json::from_str(&text).map_err(|e| BrokerError::new("io", e.to_string())),
            Err((_, message, data)) => Err(host_error(message, &data)),
        }
    }
}

/// Host 的串口业务失败经 JSON-RPC `error.data.code` 带回；缺失时按读写错误处理（TS toBrokerError 同）。
fn host_error(message: String, data: &Value) -> BrokerError {
    BrokerError { code: data["code"].as_str().unwrap_or("io").to_owned(), message }
}

/// wait_for 被放弃（对端断开导致 select 丢弃 future）时通知 Host 按 requestId 结束等待并释放订阅。
struct CancelOnDrop {
    host: Option<EventSink>,
    params: Option<Value>,
}
impl CancelOnDrop {
    fn disarm(mut self) -> Self {
        self.params = None;
        self
    }
}
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        let (Some(host), Some(params)) = (self.host.take(), self.params.take()) else { return };
        tokio::spawn(async move {
            let (reply, _receive) = tokio::sync::oneshot::channel();
            let _ = host.send(Event::HostRequest { method: CANCEL_METHOD.into(), params, reply }).await;
        });
    }
}

async fn read_line<S: AsyncRead + Unpin>(stream: &mut S) -> Result<String, BrokerError> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let count = stream.read(&mut chunk).await.map_err(|e| BrokerError::new("io", e.to_string()))?;
        if count == 0 {
            return Err(BrokerError::new("io", "Serial broker connection closed"));
        }
        buffer.extend_from_slice(&chunk[..count]);
        if buffer.len() > MAX_REQUEST_BYTES {
            return Err(BrokerError::new("invalidInput", "Serial broker request exceeded 1 MiB"));
        }
        if let Some(end) = buffer.iter().position(|b| *b == b'\n') {
            return String::from_utf8(buffer[..end].to_vec()).map_err(|e| BrokerError::new("invalidInput", e.to_string()));
        }
    }
}

async fn closed<R: AsyncRead + Unpin>(mut reader: R) {
    let mut byte = [0u8; 1];
    loop {
        match reader.read(&mut byte).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
    }
}

#[cfg(test)]
#[path = "serial_broker_tests.rs"]
mod tests;
