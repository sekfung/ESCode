//! node_repl 浏览器 broker（docs/specs/rust-browser-use.md 第 2 期），对齐 TS `node-repl-browser-broker.ts` 与
//! `browser-control-broker.ts`：本进程监听私有 socket（Unix socket / Windows 命名管道），校验 token 与 runtime scope，
//! 把 `list`/`execute` 转成 Host 的 `interaction/browserList` / `interaction/browserExecute`；对端断开即取消在途请求。
use crate::contract::{Event, EventSink};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex, OnceLock},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub(super) const SOCKET_ENV: &str = "ZCODE_NODE_REPL_BROWSER_BROKER_SOCKET";
pub(super) const TOKEN_ENV: &str = "ZCODE_NODE_REPL_BROWSER_BROKER_TOKEN";
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const SUBAGENT_MESSAGE: &str = "Browser is not available in subagent";

pub(super) struct Broker {
    pub socket: String,
    pub token: String,
    host: Arc<OnceLock<EventSink>>,
    /// 会话的请求上下文（来自该会话 MCP 调用的 `_meta`）：Host 请求的 workspace 与 clientMode 取自这里。
    sessions: Mutex<BTreeMap<String, Value>>,
    /// 本轮轮尾截图候选（TS browser-turn-state：node_repl 结果 `_meta["zcode/browserTurnScreenshot"]`），按 (session, turn)。
    candidates: Mutex<BTreeMap<(String, String), (String, u64)>>,
    /// runtime 自己操作过的 (browserId, generation)（轮尾截图）：此后每个 turn 结束发 turnEnded、关闭会话发
    /// closeSession。TS 中 node_repl broker 与会话 runtime 各持一个 BrowserControlPort，生命周期只覆盖后者的连接。
    connections: Mutex<BTreeMap<String, BTreeSet<(String, u64)>>>,
}

impl Broker {
    pub fn start(host: Arc<OnceLock<EventSink>>) -> Option<Arc<Self>> {
        let id = uuid::Uuid::new_v4();
        let socket = if cfg!(windows) {
            format!(r"\\.\pipe\zcode-node-repl-{id}")
        } else {
            std::env::temp_dir().join(format!("znr-{id}.sock")).to_string_lossy().into_owned()
        };
        let token = super::mcp_oauth_flow::hex(&zcode_cli_host::credential_cipher::random_bytes(32));
        let broker = Arc::new(Self {
            socket,
            token,
            host,
            sessions: Mutex::default(),
            candidates: Mutex::default(),
            connections: Mutex::default(),
        });
        super::browser_broker_listen::listen(broker.clone()).ok()?;
        Some(broker)
    }
    pub fn remember(&self, session: &str, meta: &Value) {
        if meta.is_object() {
            self.sessions.lock().unwrap().insert(session.into(), meta.clone());
        }
    }
    pub fn record_turn_screenshot(&self, session: &str, turn: &str, candidate: &Value) {
        let (Some(browser), Some(generation)) = (candidate["browserId"].as_str(), candidate["browserGeneration"].as_u64()) else {
            return;
        };
        self.candidates.lock().unwrap().insert((session.into(), turn.into()), (browser.into(), generation));
    }
    pub fn env(&self) -> Value {
        json!({SOCKET_ENV: self.socket, TOKEN_ENV: self.token})
    }

    /// 单个连接：读一行请求，执行并回写一行响应（TS `handleSocket`）。
    pub async fn serve<S: AsyncRead + AsyncWrite + Unpin>(self: Arc<Self>, mut stream: S) {
        let mut id = uuid::Uuid::new_v4().to_string();
        let result = async {
            let line = read_line(&mut stream).await?;
            let payload: Value = serde_json::from_str(&line)?;
            // 先取请求 id：参数错误也要带原 id 回给 node_repl，避免被 id mismatch 二次错误吞掉。
            if let Some(value) = payload["id"].as_str().filter(|v| uuid::Uuid::parse_str(v).is_ok()) {
                id = value.to_owned();
            }
            self.authorize(&payload)?;
            // 执行期间对端断开（steer 取消）即取消 Host 请求。
            let (reader, mut writer) = tokio::io::split(&mut stream);
            let response = tokio::select! {
                response = self.execute(&id, &payload) => response?,
                _ = closed(reader) => anyhow::bail!("Node REPL browser broker peer disconnected"),
            };
            writer.write_all(format!("{response}\n").as_bytes()).await?;
            anyhow::Ok(())
        }
        .await;
        if let Err(error) = result {
            let response = json!({"id": id, "ok": false, "error": error.to_string()});
            let _ = stream.write_all(format!("{response}\n").as_bytes()).await;
        }
        let _ = stream.shutdown().await;
    }

    fn authorize(&self, request: &Value) -> anyhow::Result<()> {
        let token = request["token"].as_str().unwrap_or_default();
        let (actual, expected) = (token.as_bytes(), self.token.as_bytes());
        let equal = actual.len() == expected.len()
            && actual.iter().zip(expected).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0;
        anyhow::ensure!(equal, "Node REPL browser broker request is not authorized");
        // 共享 node_repl 子进程不绑定会话；subagent 在到达 Host 前直接拒绝（TS authorizeRequest）。
        anyhow::ensure!(request["runtimeScope"] != "subagent", SUBAGENT_MESSAGE);
        Ok(())
    }

    async fn execute(&self, id: &str, request: &Value) -> anyhow::Result<Value> {
        let session = request["sessionId"].as_str().filter(|s| !s.trim().is_empty());
        let session = session.ok_or_else(|| anyhow::anyhow!("Browser request is missing sessionId"))?;
        let turn = request["turnId"].as_str();
        let mut params = self.context(session, turn)?;
        match request["op"].as_str() {
            Some("list") => {
                let result = self.host_request("interaction/browserList", params).await?;
                Ok(json!({"id": id, "ok": true, "browsers": result["browsers"]}))
            }
            Some("execute") => {
                let browser = request["browserId"].as_str().filter(|b| !b.trim().is_empty());
                let browser = browser.ok_or_else(|| anyhow::anyhow!("Browser request is missing browserId"))?;
                let generation = request["browserGeneration"].as_u64().unwrap_or(0);
                params["browserId"] = browser.into();
                params["browserGeneration"] = generation.into();
                params["command"] = request["command"].clone();
                let request_id = params["requestId"].clone();
                let cancel = CancelOnDrop { broker: self, params: Some(params.clone()), request_id };
                let result = self.host_request("interaction/browserExecute", params).await;
                std::mem::forget(cancel.disarm());
                Ok(json!({"id": id, "ok": true, "result": result?}))
            }
            _ => anyhow::bail!("Invalid Node REPL browser broker request"),
        }
    }

    /// TS `buildBrowserRequestContext`：workspaceKey 优先 identity，clientMode 取会话订阅形态。
    fn context(&self, session: &str, turn: Option<&str>) -> anyhow::Result<Value> {
        let sessions = self.sessions.lock().unwrap();
        let meta = sessions.get(session).ok_or_else(|| anyhow::anyhow!("Session not found: {session}"))?;
        let mut params = json!({
            "requestId": uuid::Uuid::new_v4().to_string(),
            "sessionId": session,
            "workspaceKey": meta["workspace_key"],
            "workspacePath": meta["workspace_path"],
            "clientMode": meta["client_mode"].as_str().unwrap_or("desktop-continuous"),
            "sessionContext": "live",
        });
        if let Some(turn) = turn.or_else(|| meta["turn_id"].as_str()) {
            params["turnId"] = turn.into();
        }
        if let Some(identity) = meta["workspace_identity"].as_str() {
            params["workspaceIdentity"] = identity.into();
        }
        Ok(params)
    }

    async fn host_request(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        let host = self.host.get().ok_or_else(|| anyhow::anyhow!("Browser host is unavailable"))?;
        let (reply, receive) = tokio::sync::oneshot::channel();
        host.send(Event::HostRequest { method: method.into(), params, reply }).await?;
        match receive.await? {
            Ok(text) => Ok(serde_json::from_str(&text)?),
            Err((_, message)) => anyhow::bail!(message),
        }
    }

    /// TS `appendBrowserTurnScreenshot`：本轮有候选时对 active tab 截图，超 200 KiB 按统一图片处理压缩，
    /// 返回 `browser_turn_end` 图片卡；无候选、无 active tab 或截图失败返回 None。
    pub async fn turn_screenshot(&self, session: &str, turn: &str) -> Option<Value> {
        let (browser, generation) = self.candidates.lock().unwrap().remove(&(session.into(), turn.into()))?;
        self.connections.lock().unwrap().entry(session.into()).or_default().insert((browser.clone(), generation));
        let command = |command: Value| {
            let mut params = self.context(session, Some(turn)).ok()?;
            params["browserId"] = browser.clone().into();
            params["browserGeneration"] = generation.into();
            params["command"] = command;
            Some(params)
        };
        let listed = self.host_request("interaction/browserExecute", command(json!({"method": "list"}))?).await.ok()?;
        let tabs = listed["tabs"].as_array().filter(|_| listed["ok"] == true)?;
        let active = tabs.iter().find(|t| t["active"] == true)?;
        let captured = self
            .host_request("interaction/browserExecute", command(json!({"method": "screenshot", "tabId": active["tabId"]}))?)
            .await
            .ok()?;
        let image = captured["image"].as_object().filter(|_| captured["ok"] == true)?;
        let (mut base64, mut mime) = (image.get("base64")?.as_str()?.to_owned(), image.get("mimeType")?.as_str()?.to_owned());
        if base64.len() > crate::domain::tool_display::MAX_NODE_REPL_IMAGE_BASE64_BYTES {
            (base64, mime) = compress(&base64, &mime)?;
        }
        let mut display = crate::domain::tool_display::node_repl_images(&json!({"images": [{"base64": base64, "mimeType": mime}]}))?;
        display["source"] = "browser_turn_end".into();
        Some(display)
    }

    /// turn 结束 / 关闭会话：对 runtime 操作过的每个 browser 发生命周期命令（TS `sendLifecycle`，失败不影响主流程）。
    pub async fn lifecycle(&self, session: &str, turn: Option<&str>, close: bool) {
        let connections: Vec<_> = if close {
            self.connections.lock().unwrap().remove(session).into_iter().flatten().collect()
        } else {
            self.connections.lock().unwrap().get(session).into_iter().flatten().cloned().collect()
        };
        for (browser, generation) in connections {
            let Ok(mut params) = self.context(session, turn) else { continue };
            params["browserId"] = browser.into();
            params["browserGeneration"] = generation.into();
            params["command"] = if close {
                json!({"method": "closeSession"})
            } else {
                json!({"method": "turnEnded", "turnId": turn})
            };
            let _ = self.host_request("interaction/browserExecute", params).await;
        }
        if close {
            self.sessions.lock().unwrap().remove(session);
        }
    }
}

/// 取消在途 execute：被丢弃（对端断开导致 select 放弃）时，按同一 backend/generation 发 `cancelRequest`。
struct CancelOnDrop<'a> {
    broker: &'a Broker,
    params: Option<Value>,
    request_id: Value,
}
impl CancelOnDrop<'_> {
    fn disarm(mut self) -> Self {
        self.params = None;
        self
    }
}
impl Drop for CancelOnDrop<'_> {
    fn drop(&mut self) {
        let Some(mut params) = self.params.take() else { return };
        let Some(host) = self.broker.host.get().cloned() else { return };
        params["requestId"] = uuid::Uuid::new_v4().to_string().into();
        params["command"] = json!({"method": "cancelRequest", "requestId": self.request_id});
        tokio::spawn(async move {
            let (reply, _receive) = tokio::sync::oneshot::channel();
            let _ = host.send(Event::HostRequest { method: "interaction/browserExecute".into(), params, reply }).await;
        });
    }
}

async fn read_line<S: AsyncRead + Unpin>(stream: &mut S) -> anyhow::Result<String> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let count = stream.read(&mut chunk).await?;
        anyhow::ensure!(count > 0, "Node REPL browser broker connection closed");
        buffer.extend_from_slice(&chunk[..count]);
        anyhow::ensure!(buffer.len() <= MAX_REQUEST_BYTES, "Node REPL browser broker request exceeded 1 MiB");
        if let Some(end) = buffer.iter().position(|b| *b == b'\n') {
            return Ok(String::from_utf8(buffer[..end].to_vec())?);
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

/// 复杂页面的原始 PNG 常超过 node_repl 的 200 KiB 展示预算：按 TS 同一预算（最长边 2048、无 token 上限）压缩。
fn compress(base64_image: &str, mime: &str) -> Option<(String, String)> {
    use base64::Engine as _;
    let engine = base64::engine::general_purpose::STANDARD;
    let bytes = engine.decode(base64_image).ok()?;
    let max = crate::domain::tool_display::MAX_NODE_REPL_IMAGE_BASE64_BYTES;
    let budget = zcode_cli_host::image_budget::Budget {
        max_base64: max,
        max_raw: max * 3 / 4,
        max_tokens: None,
        max_dimension: 2048,
    };
    let prepared = zcode_cli_host::image_budget::prepare_within(bytes, mime, &budget).ok()?;
    Some((engine.encode(prepared.data), prepared.media_type.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn broker() -> Broker {
        Broker {
            socket: String::new(),
            token: "a".repeat(64),
            host: Arc::default(),
            sessions: Mutex::default(),
            candidates: Mutex::default(),
            connections: Mutex::default(),
        }
    }

    #[test]
    fn rejects_wrong_token_and_subagent_scope() {
        let broker = broker();
        let good = json!({"token": "a".repeat(64), "runtimeScope": "main"});
        assert!(broker.authorize(&good).is_ok());
        let wrong = json!({"token": "b".repeat(64), "runtimeScope": "main"});
        assert!(broker.authorize(&wrong).unwrap_err().to_string().contains("not authorized"));
        let short = json!({"token": "a", "runtimeScope": "main"});
        assert!(broker.authorize(&short).is_err());
        let subagent = json!({"token": "a".repeat(64), "runtimeScope": "subagent"});
        assert_eq!(broker.authorize(&subagent).unwrap_err().to_string(), SUBAGENT_MESSAGE);
    }

    #[test]
    fn unknown_session_is_rejected_before_reaching_the_host() {
        let broker = broker();
        assert!(broker.context("sess_unknown", None).is_err());
        broker.remember("sess_1", &json!({"workspace_key": "k", "workspace_path": "p", "client_mode": "web-remote-replayable", "turn_id": "t1"}));
        let params = broker.context("sess_1", None).unwrap();
        assert_eq!(params["workspaceKey"], "k");
        assert_eq!(params["clientMode"], "web-remote-replayable");
        assert_eq!(params["turnId"], "t1");
        assert_eq!(params["sessionContext"], "live");
    }
}
