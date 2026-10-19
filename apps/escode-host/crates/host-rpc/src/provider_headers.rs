//! Provider runtime headers 反向请求（TS `escodeAgentService` 的 pendingProviderRuntimeHeaders）。
//!
//! 运行时发 `interaction/requestProviderRuntimeHeaders {requestId, workspace, ...}`；Host 把请求
//! 广播给 Renderer，等 `respondProviderRuntimeHeaders({requestId, response})` 后把
//! `{headersApplied, requestAuth|errorMessage}` 作为原 id 的应答写回运行时。
//!
//! 与 TS 的差异：Host 不做账号 IO；Renderer 提供的 `runtimeProviderHeaders` 直接作为
//! `requestAuth.headers` 下发（见 rust-u2-agent-v4.md 登记）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::{broadcast, oneshot};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

pub struct ProviderHeaders {
    pending: Mutex<HashMap<String, oneshot::Sender<Value>>>,
    events: broadcast::Sender<Value>,
}

impl ProviderHeaders {
    pub fn new() -> Arc<Self> {
        let (events, _) = broadcast::channel(64);
        Arc::new(Self {
            pending: Mutex::default(),
            events,
        })
    }

    /// 运行时请求：登记 pending、广播给 Renderer、等待应答。
    pub async fn request(self: &Arc<Self>, params: Value) -> Value {
        let Some(request_id) = params["requestId"].as_str().map(str::to_owned) else {
            return json!({"headersApplied": false, "errorMessage": "missing requestId"});
        };
        let (sender, receiver) = oneshot::channel();
        self.lock().insert(request_id.clone(), sender);
        if self.events.send(params).is_err() {
            self.lock().remove(&request_id);
            return json!({
                "headersApplied": false,
                "errorMessage": "no renderer is listening for provider runtime headers",
            });
        }
        match tokio::time::timeout(REQUEST_TIMEOUT, receiver).await {
            Ok(Ok(response)) => response,
            _ => {
                self.lock().remove(&request_id);
                json!({"headersApplied": false, "errorMessage": "provider runtime headers response timed out"})
            }
        }
    }

    /// Renderer 应答：把 UI 形状转成运行时 `requestAuth` 形状。
    pub fn respond(&self, request_id: &str, response: &Value) -> Result<(), String> {
        let Some(sender) = self.lock().remove(request_id) else {
            return Err(format!(
                "ESCode provider runtime headers request not found: {request_id}"
            ));
        };
        let runtime_response = if response["headersApplied"] == Value::Bool(true) {
            json!({
                "headersApplied": true,
                "requestAuth": {
                    "headers": response
                        .get("runtimeProviderHeaders")
                        .cloned()
                        .unwrap_or_else(|| json!({})),
                },
            })
        } else {
            let mut value = json!({"headersApplied": false});
            if let Some(message) = response
                .get("errorMessage")
                .and_then(Value::as_str)
                .filter(|message| !message.is_empty())
            {
                value["errorMessage"] = Value::String(message.to_owned());
            }
            value
        };
        let _ = sender.send(runtime_response);
        Ok(())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.events.subscribe()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, oneshot::Sender<Value>>> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn request_without_subscriber_fails_closed() {
        let headers = ProviderHeaders::new();
        let response = headers
            .request(json!({"requestId": "r1", "workspace": {"workspaceKey": "/w"}}))
            .await;
        assert_eq!(response["headersApplied"], false);
        assert!(
            response["errorMessage"]
                .as_str()
                .unwrap()
                .contains("no renderer")
        );
    }

    #[tokio::test]
    async fn renderer_response_is_converted_to_request_auth() {
        let headers = ProviderHeaders::new();
        let mut events = headers.subscribe();
        let request = {
            let headers = Arc::clone(&headers);
            tokio::spawn(async move {
                headers
                    .request(json!({"requestId": "r2", "workspace": {"workspaceKey": "/w"}}))
                    .await
            })
        };
        let event = events.recv().await.unwrap();
        assert_eq!(event["requestId"], "r2");
        headers
            .respond(
                "r2",
                &json!({"headersApplied": true, "runtimeProviderHeaders": {"x-api-key": "k"}}),
            )
            .unwrap();
        let response = request.await.unwrap();
        assert_eq!(response["headersApplied"], true);
        assert_eq!(response["requestAuth"]["headers"]["x-api-key"], "k");
        assert!(headers.respond("missing", &json!({})).is_err());
    }
}
