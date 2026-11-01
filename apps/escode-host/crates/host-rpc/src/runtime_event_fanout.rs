//! runtime → 窗口的 live 事件扇出（`v4/cua/permission-observation`、
//! `v4/plugin-ui/tool-result`）。
//!
//! TS `escodeAgentService` 从 runtime 通知流里转发这两类事件（只限本地 workspace）。
//! Host 的 `Instances` 是唯一实例表，这里在实例首次被使用时订阅其出站帧，按 method 与
//! workspace 过滤；事件不落盘、不补发，没有订阅者时直接丢弃。

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use escode_host_runtime::Instance;
use futures_util::stream;
use serde_json::Value;
use tokio::sync::broadcast;

use crate::channel::EventStream;
use crate::wire::Wire;

pub(crate) struct RuntimeEventFanout {
    method: &'static str,
    validate: fn(&Value) -> bool,
    sender: broadcast::Sender<Value>,
    pumped: Mutex<BTreeSet<(String, u64)>>,
}

impl RuntimeEventFanout {
    pub(crate) fn new(method: &'static str, validate: fn(&Value) -> bool) -> Self {
        Self {
            method,
            validate,
            sender: broadcast::channel(256).0,
            pumped: Mutex::default(),
        }
    }

    pub(crate) async fn attach(&self, instance: &Arc<Instance>) {
        let key = (instance.identity.clone(), instance.generation);
        {
            let mut pumped = self
                .pumped
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !pumped.insert(key) {
                return;
            }
        }
        let mut frames = instance.subscribe().await;
        let sender = self.sender.clone();
        let method = self.method;
        let validate = self.validate;
        let path = instance.workspace_path().to_owned();
        // TS 只转发本地 workspace 的事件；远程 workspace 由远端 Host 自己上报。
        let identity = instance
            .workspace_identity()
            .map(str::to_owned)
            .filter(|value| !crate::window_controller::is_remote_workspace_identity(value));
        tokio::spawn(async move {
            while let Some(line) = frames.recv().await {
                let Ok(message) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if message["method"] != method {
                    continue;
                }
                // 形状必须在校验后才补 workspace 字段：renderer 副作用不能吃未知字段。
                let mut event = message["params"].clone();
                if !validate(&event) {
                    continue;
                }
                event["workspacePath"] = Value::from(path.clone());
                if let Some(identity) = &identity {
                    event["workspaceIdentity"] = Value::from(identity.clone());
                }
                let _ = sender.send(event);
            }
        });
    }

    pub(crate) fn event(&self, workspace: Option<String>) -> EventStream {
        let receiver = self.sender.subscribe();
        Box::pin(stream::unfold(
            (receiver, workspace),
            |(mut receiver, workspace)| async move {
                loop {
                    match receiver.recv().await {
                        Ok(value) => {
                            let matches = workspace.as_deref().is_none_or(|target| {
                                value["workspaceKey"].as_str() == Some(target)
                                    || value["workspacePath"].as_str() == Some(target)
                                    || value["workspaceIdentity"].as_str() == Some(target)
                            });
                            if matches {
                                return Some((Wire::Object(value), (receiver, workspace)));
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => return None,
                    }
                }
            },
        ))
    }
}

pub(crate) fn cua_observation(params: &Value) -> bool {
    params.is_object()
}

/// 对齐 shared `pluginUiToolResultNotificationSchema`（strict）：远端/迟到帧里的未知字段
/// 一律丢弃，避免 renderer 用未知载荷创建页面。
pub(crate) fn plugin_ui_tool_result(params: &Value) -> bool {
    let Some(object) = params.as_object() else {
        return false;
    };
    // `pluginUiToolResultNotificationSchema` 是 strict：宿主补 workspace 字段前先拒绝未知键。
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "sessionId" | "toolCallId" | "serverName" | "ui" | "input"
        )
    }) {
        return false;
    }
    let text = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
    };
    if !(text("sessionId") && text("toolCallId") && text("serverName")) {
        return false;
    }
    if object.get("input").is_some_and(|input| !input.is_object()) {
        return false;
    }
    object.get("ui").is_some_and(valid_ui)
}

fn valid_ui(ui: &Value) -> bool {
    let Some(ui) = ui.as_object() else {
        return false;
    };
    const KEYS: [&str; 13] = [
        "pluginId",
        "resourceUri",
        "preferredDisplayMode",
        "prefersBorder",
        "csp",
        "structuredContent",
        "widgetMeta",
        "content",
        "truncated",
        "truncatedBytes",
        "isError",
        "showInline",
        "surface",
    ];
    if ui.keys().any(|key| !KEYS.contains(&key.as_str())) {
        return false;
    }
    let text = |key: &str| {
        ui.get(key)
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
    };
    if !(text("pluginId") && text("resourceUri")) {
        return false;
    }
    let mode_ok = ui
        .get("preferredDisplayMode")
        .is_none_or(|value| matches!(value.as_str(), Some("inline" | "fullscreen" | "pip")));
    let strings_ok = ["structuredContent", "widgetMeta", "content", "surface"]
        .iter()
        .all(|key| ui.get(*key).is_none_or(Value::is_string));
    let bools_ok = ["prefersBorder", "truncated", "showInline"]
        .iter()
        .all(|key| ui.get(*key).is_none_or(Value::is_boolean));
    // `mcpToolDisplayUiSchema.refine(ui => !ui.isError)`：出现 isError 一律不投递。
    ui.get("isError").is_none()
        && mode_ok
        && strings_ok
        && ui.get("csp").is_none_or(Value::is_object)
        && bools_ok
        && ui
            .get("truncatedBytes")
            .is_none_or(|value| value.as_u64().is_some())
}

#[cfg(test)]
mod tests {
    use super::{cua_observation, plugin_ui_tool_result};
    use serde_json::json;

    fn live() -> serde_json::Value {
        json!({
            "sessionId": "session-1",
            "toolCallId": "tool",
            "serverName": "plugin:editor:editor",
            "ui": {"pluginId": "editor", "resourceUri": "ui://editor", "surface": "editor"},
            "input": {"file": "a.png"}
        })
    }

    #[test]
    fn accepts_the_live_notification_shape() {
        assert!(plugin_ui_tool_result(&live()));
        assert!(cua_observation(&json!({"toolCallId": "x"})));
    }

    #[test]
    fn rejects_error_unknown_fields_and_missing_identity() {
        let mut errored = live();
        errored["ui"]["isError"] = true.into();
        assert!(!plugin_ui_tool_result(&errored));

        let mut unknown = live();
        unknown["extra"] = true.into();
        assert!(!plugin_ui_tool_result(&unknown));

        let mut unknown_ui = live();
        unknown_ui["ui"]["heightHint"] = 10.into();
        assert!(!plugin_ui_tool_result(&unknown_ui));

        let mut missing = live();
        missing["sessionId"] = "".into();
        assert!(!plugin_ui_tool_result(&missing));

        let mut array_input = live();
        array_input["input"] = json!([1]);
        assert!(!plugin_ui_tool_result(&array_input));
    }
}
