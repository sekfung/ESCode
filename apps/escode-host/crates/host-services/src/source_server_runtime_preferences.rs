//! 远端 Agent 的 runtime preferences 应答（S4-g，docs/specs/source-server-remote-rust-host.md §11.7）。
//!
//! `desktop-attached-remote` 下 Agent 在远端运行，但设置权威仍在本地 Host。远端 Agent 通过
//! `escode-agent.onDynamicSessionRuntimePreferencesRequest` 请求偏好；这里读取本地 settings 并原路
//! 应答。解析函数与本地 `session/requestRuntimePreferences` 共用，避免默认值漂移。

use std::path::PathBuf;
use std::sync::Arc;

use escode_host_control::logging;
use escode_host_core::rpc_serialization::WireValue;
use serde_json::{Map, Value, json};

use crate::source_server_client::RemoteChannelClient;

const AGENT_CHANNEL: &str = "escode-agent";
const REQUEST_EVENT: &str = "onDynamicSessionRuntimePreferencesRequest";
const RESPOND_METHOD: &str = "respondSessionRuntimePreferences";

/// TS `DEFAULT_ESCODE_MODEL_CONTEXT_BUDGET_STRATEGY`。
const MODEL_CONTEXT_BUDGET_STRATEGY: &str = "preflight-v1";

struct Request {
    request_id: String,
    session_id: String,
    scope: String,
}

/// 为一条已初始化连接注册监听；旧远端未广播 `escode-agent` 时跳过。
pub fn attach(client: &Arc<RemoteChannelClient>, home: PathBuf) {
    if !client.has_channel(AGENT_CHANNEL) {
        return;
    }
    let client = Arc::clone(client);
    tokio::spawn(async move { serve(client, home).await });
}

async fn serve(client: Arc<RemoteChannelClient>, home: PathBuf) {
    let (listener, mut events) = match client.listen(AGENT_CHANNEL, REQUEST_EVENT, None).await {
        Ok(listener) => listener,
        Err(error) => {
            logging::warn(
                "source-server",
                format!("runtime preferences listener unavailable: {error:#}"),
            );
            return;
        }
    };
    // 连接 dispose 时事件发送端随 events map 丢弃，这里自然收到 None 并退出。
    while let Some(event) = events.recv().await {
        let Some(payload) = event.to_json() else {
            logging::warn(
                "source-server",
                "ignored non-JSON session runtime preferences request",
            );
            continue;
        };
        let Some(request) = parse_request(&payload) else {
            logging::warn(
                "source-server",
                "ignored invalid session runtime preferences request",
            );
            continue;
        };
        let response = resolve(&home, &request).await;
        // TS `ProxyChannel.call(ctx, command, args)` 用 `apply(handler, args)` 展开实参，信封必须
        // 是单元素数组；裸对象会被真实 server 当成多个/缺失参数。
        let argument = WireValue::Array(vec![WireValue::from_json(&response)]);
        if let Err(error) = client
            .call(AGENT_CHANNEL, RESPOND_METHOD, Some(argument))
            .await
        {
            logging::warn(
                "source-server",
                format!("runtime preferences response failed: {error}"),
            );
            break;
        }
    }
    client.dispose_listener(listener);
}

async fn resolve(home: &std::path::Path, request: &Request) -> Value {
    let resolution = match crate::settings::read(home).await {
        Ok(settings) => json!({
            "status": "resolved",
            "preferences": preferences(&settings, &request.scope),
        }),
        Err(error) => json!({
            "status": "failed",
            "message": format!("{error:#}"),
        }),
    };
    logging::debug(
        "source-server",
        format!(
            "runtime preferences response prepared: session={} request={}",
            request.session_id, request.request_id
        ),
    );
    json!({"requestId": request.request_id, "resolution": resolution})
}

/// 动态事件体：`{requestId, sessionId, scope}`；形状不符时不猜测。
fn parse_request(event: &Value) -> Option<Request> {
    let record = event.as_object()?;
    let text = |key: &str| {
        record
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let scope = text("scope")?;
    if !matches!(scope.as_str(), "runtime-materialization" | "user-execution") {
        return None;
    }
    Some(Request {
        request_id: text("requestId")?,
        session_id: text("sessionId")?,
        scope,
    })
}

/// TS `resolveSessionRuntimePreferences` + 结果 schema 的缺省值（本地与远端共用）。
pub fn preferences(settings: &Value, scope: &str) -> Value {
    let mut result = Map::new();
    result.insert(
        "askUserQuestionAutoResolutionEnabled".to_owned(),
        Value::Bool(settings["askUserQuestionAutoResolutionEnabled"] != false),
    );
    result.insert(
        "nativeSearchEnhancementsEnabled".to_owned(),
        Value::Bool(settings["nativeSearchEnhancementsEnabled"] != false),
    );
    result.insert(
        "memoryEnabled".to_owned(),
        Value::Bool(settings["memoryEnabled"] == true),
    );
    result.insert(
        "modelContextBudgetStrategy".to_owned(),
        json!(MODEL_CONTEXT_BUDGET_STRATEGY),
    );
    if scope == "user-execution"
        && let Some(shell) = valid_shell(&settings["integratedTerminalShell"])
    {
        result.insert("integratedTerminalShell".to_owned(), shell);
    }
    // TS 在本地 workspace 为 true（Host 能应答 `subagents/readRuntimeConfig`）。subagents 域未迁移前
    // 回 false，runtime 退回内置子代理（登记差异：rust-u2-agent-v4.md §6-2）。
    result.insert(
        "subagentRuntimeConfigEnabled".to_owned(),
        Value::Bool(false),
    );
    Value::Object(result)
}

/// TS `integratedTerminalShellSelectionSchema`：`{mode:"auto"}` 或
/// `{mode:"shell", dialect:"cmd"|"git-bash", id, label, path}`（非空串）。形状不符按缺省处理。
pub fn valid_shell(value: &Value) -> Option<Value> {
    let record = value.as_object()?;
    match record.get("mode").and_then(Value::as_str)? {
        "auto" => Some(json!({"mode": "auto"})),
        "shell" => {
            let dialect = record.get("dialect").and_then(Value::as_str)?;
            if !matches!(dialect, "cmd" | "git-bash") {
                return None;
            }
            let text = |key: &str| {
                record
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
            };
            Some(json!({
                "mode": "shell",
                "dialect": dialect,
                "id": text("id")?,
                "label": text("label")?,
                "path": text("path")?,
            }))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_shape_is_strict() {
        let request = parse_request(&json!({
            "requestId": "r1",
            "sessionId": "s1",
            "scope": "user-execution"
        }))
        .expect("valid request");
        assert_eq!(request.request_id, "r1");
        assert!(
            parse_request(&json!({"requestId": "r1", "sessionId": "s1", "scope": "bad"})).is_none()
        );
        assert!(
            parse_request(&json!({"requestId": "", "sessionId": "s1", "scope": "user-execution"}))
                .is_none()
        );
    }

    #[test]
    fn defaults_follow_the_ts_resolver() {
        assert_eq!(
            preferences(&json!({}), "runtime-materialization"),
            json!({
                "askUserQuestionAutoResolutionEnabled": true,
                "nativeSearchEnhancementsEnabled": true,
                "memoryEnabled": false,
                "modelContextBudgetStrategy": "preflight-v1",
                "subagentRuntimeConfigEnabled": false,
            })
        );
    }
}
