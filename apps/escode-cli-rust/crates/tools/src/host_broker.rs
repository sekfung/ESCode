//! 宿主工具 broker：serial（docs/specs/serial-agent-tools.md）使用，
//! 对齐 TS `serial-broker.ts` 与对应的 `serial-control-broker.ts`：
//! 监听私有 socket，校验 token 与 runtime scope，把宿主 MCP server 的请求转成 Host 的 `interaction/*` 反向请求。
//! 会话归属以 hub 记住的该会话 MCP 调用 `_meta` 为准；参数的严格校验由 Host 完成，这里只校验形状。
//! 各类宿主工具只在方法表、错误码与取消方法上不同，由 [`Spec`] 描述。
use crate::contract::{Event, EventSink};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, OnceLock},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const MAX_REQUEST_BYTES: usize = 1024 * 1024;

/// 一种宿主工具 broker 的差异部分。
pub(super) struct Spec {
    /// 错误消息里的名字（Serial）。
    label: &'static str,
    /// Windows 命名管道前缀与 Unix socket 文件前缀（与 TS 同名，便于排查）。
    pipe_prefix: &'static str,
    file_prefix: &'static str,
    socket_env: &'static str,
    token_env: &'static str,
    /// broker op → Host 反向请求方法。
    methods: &'static [(&'static str, &'static str)],
    /// 需要在调用被放弃时通知 Host 的长等待 op 及其取消方法；短请求为 None。
    cancel: Option<(&'static str, &'static str)>,
    unauthorized_code: &'static str,
    /// 连接失败、或 Host 失败未带 `error.data.code` 时的错误码。
    io_code: &'static str,
    too_large_code: &'static str,
    subagent_message: &'static str,
}

pub(super) static SERIAL: Spec = Spec {
    label: "Serial",
    pipe_prefix: "escode-serial",
    file_prefix: "zsr",
    socket_env: "ESCODE_SERIAL_BROKER_SOCKET",
    token_env: "ESCODE_SERIAL_BROKER_TOKEN",
    methods: &[
        ("list", "interaction/serialList"),
        ("open", "interaction/serialOpen"),
        ("write", "interaction/serialWrite"),
        ("read", "interaction/serialRead"),
        ("waitFor", "interaction/serialWaitFor"),
        ("close", "interaction/serialClose"),
        ("setSignals", "interaction/serialSetSignals"),
    ],
    cancel: Some(("waitFor", "interaction/serialCancel")),
    unauthorized_code: "unauthorized",
    io_code: "io",
    too_large_code: "invalidInput",
    // 子 agent 不得操作硬件：避免多个子 agent 同时往同一串口写入（TS authorize 同）。
    subagent_message: "Serial port tools are not available in subagents",
};

/// broker 失败：code 原样作为工具错误码返回给 MCP server（如 busy、notOpen、unavailable）。
#[derive(Debug, PartialEq)]
struct BrokerError {
    code: String,
    message: String,
}
impl BrokerError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl Spec {
    fn method(&self, op: &str) -> Option<&'static str> {
        self.methods
            .iter()
            .find(|(name, _)| *name == op)
            .map(|(_, method)| *method)
    }
}

pub(super) struct HostBroker {
    spec: &'static Spec,
    pub socket: String,
    pub token: String,
    host: Arc<OnceLock<EventSink>>,
    /// 会话的请求上下文（该会话 MCP 调用的 `_meta`）：Host 请求的 workspace 身份取自这里。
    sessions: Mutex<BTreeMap<String, Value>>,
}

impl HostBroker {
    pub fn start(spec: &'static Spec, host: Arc<OnceLock<EventSink>>) -> Option<Arc<Self>> {
        let id = uuid::Uuid::new_v4();
        let socket = if cfg!(windows) {
            format!(r"\\.\pipe\{}-{id}", spec.pipe_prefix)
        } else {
            std::env::temp_dir()
                .join(format!("{}-{id}.sock", spec.file_prefix))
                .to_string_lossy()
                .into_owned()
        };
        let token =
            super::mcp_oauth_flow::hex(&escode_cli_host::credential_cipher::random_bytes(32));
        let broker = Arc::new(Self {
            spec,
            socket,
            token,
            host,
            sessions: Mutex::default(),
        });
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
            self.sessions
                .lock()
                .unwrap()
                .insert(session.into(), meta.clone());
        }
    }
    pub fn env(&self) -> Value {
        json!({self.spec.socket_env: self.socket, self.spec.token_env: self.token})
    }

    /// 单个连接：读一行请求，执行并回写一行响应；执行期间对端断开即放弃并通知 Host 取消等待。
    pub async fn serve<S: AsyncRead + AsyncWrite + Unpin + Send>(self: Arc<Self>, mut stream: S) {
        let mut id = uuid::Uuid::new_v4().to_string();
        let spec = self.spec;
        let outcome: Result<Value, BrokerError> = async {
            let line = read_line(spec, &mut stream).await?;
            let payload: Value =
                serde_json::from_str(&line).map_err(|e| BrokerError::new("invalidInput", e.to_string()))?;
            if let Some(value) = payload["id"].as_str().filter(|v| uuid::Uuid::parse_str(v).is_ok()) {
                id = value.to_owned();
            }
            self.authorize(&payload)?;
            let (reader, _writer) = tokio::io::split(&mut stream);
            tokio::select! {
                result = self.execute(&payload) => result,
                _ = closed(reader) => Err(BrokerError::new(spec.io_code, format!("{} broker peer disconnected", spec.label))),
            }
        }
        .await;
        let response = match outcome {
            Ok(result) => json!({"id": id, "ok": true, "result": result}),
            Err(error) => {
                json!({"id": id, "ok": false, "error": {"code": error.code, "message": error.message}})
            }
        };
        let _ = stream.write_all(format!("{response}\n").as_bytes()).await;
        let _ = stream.shutdown().await;
    }

    fn authorize(&self, request: &Value) -> Result<(), BrokerError> {
        let token = request["token"].as_str().unwrap_or_default();
        let (actual, expected) = (token.as_bytes(), self.token.as_bytes());
        let equal = actual.len() == expected.len()
            && actual
                .iter()
                .zip(expected)
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                == 0;
        if !equal {
            let message = format!("{} broker request is not authorized", self.spec.label);
            return Err(BrokerError::new(self.spec.unauthorized_code, message));
        }
        if request["runtimeScope"] == "subagent" {
            return Err(BrokerError::new("unavailable", self.spec.subagent_message));
        }
        Ok(())
    }

    /// TS `createProtocolSerialControlPort`：按会话补齐 workspace 身份，生成 Host 请求参数。
    fn params(&self, request: &Value) -> Result<(&'static str, Value), BrokerError> {
        let label = self.spec.label;
        let op = request["op"].as_str().unwrap_or_default();
        let method = self.spec.method(op).ok_or_else(|| {
            BrokerError::new(
                "invalidInput",
                format!("Unknown {} op: {op}", label.to_lowercase()),
            )
        })?;
        if !request["args"].is_object() {
            return Err(BrokerError::new(
                "invalidInput",
                format!("{label} broker request is missing args"),
            ));
        }
        let session = request["sessionId"]
            .as_str()
            .filter(|s| !s.trim().is_empty());
        let session = session.ok_or_else(|| {
            BrokerError::new(
                "invalidInput",
                format!("{label} request is missing sessionId"),
            )
        })?;
        let sessions = self.sessions.lock().unwrap();
        let meta = sessions.get(session).ok_or_else(|| {
            BrokerError::new("unavailable", format!("Session not found: {session}"))
        })?;
        let mut params = json!({
            "requestId": uuid::Uuid::new_v4().to_string(),
            "sessionId": session,
            "workspaceKey": meta["workspace_key"],
            "workspacePath": meta["workspace_path"],
            "args": request["args"],
        });
        if let Some(turn) = request["turnId"]
            .as_str()
            .or_else(|| meta["turn_id"].as_str())
        {
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
        let cancel = self.spec.cancel.filter(|(op, _)| request["op"] == *op).map(|(_, method)| CancelOnDrop {
            host: self.host.get().cloned(),
            method,
            params: Some(json!({"sessionId": params["sessionId"], "targetRequestId": params["requestId"]})),
        });
        let result = self.host_request(method, params).await;
        if let Some(cancel) = cancel {
            std::mem::forget(cancel.disarm());
        }
        result
    }

    async fn host_request(&self, method: &str, params: Value) -> Result<Value, BrokerError> {
        let host = self.host.get().ok_or_else(|| {
            BrokerError::new(
                "unavailable",
                format!("{} host is unavailable", self.spec.label),
            )
        })?;
        let (reply, receive) = tokio::sync::oneshot::channel();
        host.send(Event::HostRequest {
            method: method.into(),
            params,
            reply,
        })
        .await
        .map_err(|e| BrokerError::new(self.spec.io_code, e.to_string()))?;
        match receive
            .await
            .map_err(|e| BrokerError::new(self.spec.io_code, e.to_string()))?
        {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| BrokerError::new(self.spec.io_code, e.to_string())),
            Err((_, message, data)) => Err(host_error(self.spec, message, &data)),
        }
    }
}

/// Host 的业务失败经 JSON-RPC `error.data.code` 带回；缺失时按读写错误处理（TS toBrokerError 同）。
fn host_error(spec: &Spec, message: String, data: &Value) -> BrokerError {
    BrokerError {
        code: data["code"].as_str().unwrap_or(spec.io_code).to_owned(),
        message,
    }
}

/// wait_for 被放弃（对端断开导致 select 丢弃 future）时通知 Host 按 requestId 结束等待并释放订阅。
struct CancelOnDrop {
    host: Option<EventSink>,
    method: &'static str,
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
        let (Some(host), Some(params), method) =
            (self.host.take(), self.params.take(), self.method)
        else {
            return;
        };
        escode_cli_core_api::scope::Spawner::current().spawn("host-cancel-notify", async move {
            let (reply, _receive) = tokio::sync::oneshot::channel();
            let _ = host
                .send(Event::HostRequest {
                    method: method.into(),
                    params,
                    reply,
                })
                .await;
        });
    }
}

async fn read_line<S: AsyncRead + Unpin>(
    spec: &Spec,
    stream: &mut S,
) -> Result<String, BrokerError> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let count = stream
            .read(&mut chunk)
            .await
            .map_err(|e| BrokerError::new(spec.io_code, e.to_string()))?;
        if count == 0 {
            return Err(BrokerError::new(
                spec.io_code,
                format!("{} broker connection closed", spec.label),
            ));
        }
        buffer.extend_from_slice(&chunk[..count]);
        if buffer.len() > MAX_REQUEST_BYTES {
            let message = format!("{} broker request exceeded 1 MiB", spec.label);
            return Err(BrokerError::new(spec.too_large_code, message));
        }
        if let Some(end) = buffer.iter().position(|b| *b == b'\n') {
            return String::from_utf8(buffer[..end].to_vec())
                .map_err(|e| BrokerError::new("invalidInput", e.to_string()));
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
#[path = "host_broker_tests.rs"]
mod tests;
