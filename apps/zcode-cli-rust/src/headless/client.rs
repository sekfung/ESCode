//! `-p` 的进程内协议客户端（docs/specs/rust-headless-prompt.md）：与 app-server 同一套请求 / 通知，经内存通道直连 Engine；
//! 代替 Host 应答 Engine 的反向请求。
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::VecDeque;
use tokio::sync::mpsc;
use zcode_cli_core_api::Input;

pub struct Client {
    input: mpsc::Sender<Input>,
    output: mpsc::Receiver<Vec<Value>>,
    batch: VecDeque<Value>,
    notifications: VecDeque<Value>,
    next_id: u64,
}

impl Client {
    pub fn new(input: mpsc::Sender<Input>, output: mpsc::Receiver<Vec<Value>>) -> Self {
        Self {
            input,
            output,
            batch: VecDeque::new(),
            notifications: VecDeque::new(),
            next_id: 0,
        }
    }

    /// 下一条 Engine 输出（展开批次）；反向请求就地应答，不返回给调用方。
    async fn recv(&mut self) -> Result<Value> {
        loop {
            if let Some(message) = self.batch.pop_front() {
                if message.get("method").is_some() && message.get("id").is_some() {
                    self.answer_host_request(&message).await?;
                    continue;
                }
                return Ok(message);
            }
            let batch = self.output.recv().await.context("Runtime stopped")?;
            self.batch.extend(batch);
        }
    }

    /// 无 Host：运行时偏好回空（shell 自动探测、记忆关闭）；其余反向请求回错误（账号运行时请求头、插件 / 工作流宿主等）。
    async fn answer_host_request(&mut self, message: &Value) -> Result<()> {
        let id = match &message["id"] {
            Value::String(id) => id.clone(),
            other => other.to_string(),
        };
        let (result, error) = if message["method"] == "session/requestRuntimePreferences" {
            (json!({}), None)
        } else {
            (
                Value::Null,
                Some(json!({"code": -32601, "message": "Host is unavailable in headless mode"})),
            )
        };
        self.input
            .send(Input::Response {
                id,
                result,
                error,
                raw_result: None,
            })
            .await
            .context("Runtime stopped")
    }

    pub async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next_id += 1;
        let id = format!("headless-{}", self.next_id);
        let request =
            serde_json::from_value(json!({"id": id, "method": method, "params": params}))?;
        self.input
            .send(Input::Request(request))
            .await
            .context("Runtime stopped")?;
        loop {
            let message = self.recv().await?;
            if message.get("method").is_none() && message["id"] == id.as_str() {
                if let Some(error) = message.get("error").filter(|e| !e.is_null()) {
                    bail!("{}", error["message"].as_str().unwrap_or("Request failed"));
                }
                return Ok(message["result"].clone());
            }
            self.notifications.push_back(message);
        }
    }

    /// 下一条通知（按到达顺序）。
    pub async fn notification(&mut self) -> Result<Value> {
        if let Some(message) = self.notifications.pop_front() {
            return Ok(message);
        }
        loop {
            let message = self.recv().await?;
            if message.get("method").is_some() {
                return Ok(message);
            }
        }
    }

    /// `v4/command`：非 accepted 的 ACK 按错误返回（message 为 ACK 的说明或原因码）。
    pub async fn command(
        &mut self,
        session: Option<&str>,
        kind: &str,
        payload: Value,
    ) -> Result<Value> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let ack = self
            .request(
                "v4/command",
                json!({
                    "commandId": zcode_cli_host::id(),
                    "clientId": "headless",
                    "sessionId": session,
                    "type": kind,
                    "payload": payload,
                    "issuedAt": now,
                }),
            )
            .await?;
        if ack["status"] != "accepted" {
            let reason = ack["message"]
                .as_str()
                .or_else(|| ack["reasonCode"].as_str())
                .unwrap_or("Command rejected");
            bail!("{reason}");
        }
        Ok(ack)
    }
}
