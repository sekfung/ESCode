//! runtime→Host 反向请求的 Host 侧应答（docs/specs/rust-u2-agent-v4.md §3.3）。
//!
//! 修复（评审 A5）：此前 Rust Host 不应答任何反向请求；runtime 在物化与首个 Bash 前等待
//! `session/requestRuntimePreferences` 的回包（记忆开关、Shell 选择），该等待因此无人解除。
//!
//! TS 真相源：`escodeAgentService.ts` 的 `client.onRequest`（`sessionRuntimePreferencesAuthority:
//! "local"` 分支）+ `packages/services/src/node.ts:2217` 的 `resolveSessionRuntimePreferences`。

use std::path::PathBuf;
use std::sync::Arc;

use escode_host_control::main_broker::MainBroker;
use escode_host_runtime::{HostError, HostRequest, HostResponder};
use serde_json::{Map, Value, json};

/// Main broker 在 `rpcListen` 之后才创建；responder 先装配，槽位后填。
pub type MainBrokerSlot = Arc<std::sync::OnceLock<Arc<MainBroker>>>;

/// TS `DEFAULT_ESCODE_MODEL_CONTEXT_BUDGET_STRATEGY`。
const MODEL_CONTEXT_BUDGET_STRATEGY: &str = "preflight-v1";

/// 组合根装配的应答器：`home` 是设置域根（与 `setting` channel 同一份 settings.json）。
pub fn responder(
    home: PathBuf,
    provider_headers: Arc<crate::provider_headers::ProviderHeaders>,
    main_broker: MainBrokerSlot,
) -> HostResponder {
    let home = Arc::new(home);
    Arc::new(move |request: HostRequest| {
        let home = Arc::clone(&home);
        let provider_headers = Arc::clone(&provider_headers);
        let main_broker = Arc::clone(&main_broker);
        Box::pin(async move {
            match request.method.as_str() {
                "session/requestRuntimePreferences" => {
                    runtime_preferences(&home, &request.identity, &request.params).await
                }
                "interaction/requestProviderRuntimeHeaders" => {
                    Ok(provider_headers.request(request.params).await)
                }
                "interaction/browserList" => Ok(json!({"browsers": []})),
                "interaction/browserExecute" => {
                    browser_execute(&main_broker, &request.params).await
                }
                other => Err(HostError::unsupported(other)),
            }
        })
    })
}

async fn browser_execute(main_broker: &MainBrokerSlot, params: &Value) -> Result<Value, HostError> {
    if params.get("requestId").and_then(Value::as_str).is_none()
        || params.get("sessionId").and_then(Value::as_str).is_none()
        || params.get("command").is_none()
    {
        return Err(HostError {
            code: -32602,
            message: "Invalid interaction/browserExecute params".to_owned(),
        });
    }
    let Some(broker) = main_broker.get() else {
        return Ok(browser_unavailable("browser control not available"));
    };
    match broker
        .request("interaction/browserExecute", params.clone())
        .await
    {
        Ok(result) => Ok(result),
        Err(error) => Ok(browser_unavailable(&error)),
    }
}

fn browser_unavailable(message: &str) -> Value {
    json!({
        "ok": false,
        "error": {
            "code": "backend_unavailable",
            "message": message,
            "sideEffect": "uncertain",
        },
        "elapsedMs": 0,
    })
}

async fn runtime_preferences(
    home: &std::path::Path,
    identity: &str,
    params: &Value,
) -> Result<Value, HostError> {
    let scope = parse_params(params).ok_or_else(|| HostError {
        code: -32602,
        message: "Invalid session runtime preferences request params".to_owned(),
    })?;
    let settings = escode_host_services::settings::read(home)
        .await
        .map_err(|error| HostError {
            code: -32603,
            message: format!("{error:#}"),
        })?;
    Ok(preferences(&settings, scope, identity))
}

/// TS `escodeSessionRequestRuntimePreferencesParamsSchema`（strict：只允许 sessionId + scope）。
fn parse_params(params: &Value) -> Option<&str> {
    let record = params.as_object()?;
    if record
        .keys()
        .any(|key| key != "sessionId" && key != "scope")
    {
        return None;
    }
    record
        .get("sessionId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())?;
    record
        .get("scope")
        .and_then(Value::as_str)
        .filter(|scope| matches!(*scope, "runtime-materialization" | "user-execution"))
}

/// TS `resolveSessionRuntimePreferences` + 结果 schema 的缺省值。
pub(crate) fn preferences(settings: &Value, scope: &str, _identity: &str) -> Value {
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
fn valid_shell(value: &Value) -> Option<Value> {
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
    use escode_host_control::{ControlMessage, ControlWriter};

    #[tokio::test]
    async fn browser_execute_forwards_to_main_request() {
        let (writer, mut rx) = ControlWriter::channel(8);
        let broker = MainBroker::new(writer);
        let slot: MainBrokerSlot = Arc::new(std::sync::OnceLock::new());
        let _ = slot.set(Arc::clone(&broker));
        let responder = responder(
            PathBuf::from("/tmp"),
            crate::provider_headers::ProviderHeaders::new(),
            slot,
        );
        let call = tokio::spawn(async move {
            responder(HostRequest {
                identity: "w".to_owned(),
                workspace_path: "/w".to_owned(),
                method: "interaction/browserExecute".to_owned(),
                params: json!({
                    "requestId": "r1",
                    "sessionId": "s1",
                    "command": {"method": "navigate"},
                }),
            })
            .await
        });
        let Some(ControlMessage::MainRequest {
            request_id,
            method,
            params,
        }) = rx.recv().await
        else {
            panic!("expected mainRequest");
        };
        assert_eq!(method, "interaction/browserExecute");
        assert_eq!(params["requestId"], "r1");
        assert!(broker.resolve(&request_id, true, Some(json!({"ok": true})), None));
        assert_eq!(
            call.await.expect("task").expect("result"),
            json!({"ok": true})
        );
    }

    #[test]
    fn defaults_follow_the_ts_resolver() {
        let result = preferences(&json!({}), "runtime-materialization", "/w");
        assert_eq!(
            result,
            json!({
                "askUserQuestionAutoResolutionEnabled": true,
                "nativeSearchEnhancementsEnabled": true,
                "memoryEnabled": false,
                "modelContextBudgetStrategy": "preflight-v1",
                "subagentRuntimeConfigEnabled": false,
            })
        );
    }

    #[test]
    fn explicit_settings_and_shell_only_for_user_execution() {
        let settings = json!({
            "askUserQuestionAutoResolutionEnabled": false,
            "nativeSearchEnhancementsEnabled": false,
            "memoryEnabled": true,
            "integratedTerminalShell": {"mode": "shell", "dialect": "git-bash", "id": "g", "label": "Git Bash", "path": "C:/bash.exe"},
        });
        let materialization = preferences(&settings, "runtime-materialization", "/w");
        assert_eq!(materialization["memoryEnabled"], true);
        assert_eq!(
            materialization["askUserQuestionAutoResolutionEnabled"],
            false
        );
        assert_eq!(materialization["nativeSearchEnhancementsEnabled"], false);
        assert!(materialization.get("integratedTerminalShell").is_none());
        let execution = preferences(&settings, "user-execution", "/w");
        assert_eq!(execution["integratedTerminalShell"]["dialect"], "git-bash");
        // memoryEnabled 只认 true（TS `=== true`）。
        assert_eq!(
            preferences(&json!({"memoryEnabled": "yes"}), "user-execution", "/w")["memoryEnabled"],
            false
        );
    }

    #[test]
    fn invalid_shells_and_params_are_rejected() {
        for shell in [
            json!({"mode": "shell", "dialect": "zsh", "id": "z", "label": "Z", "path": "/bin/zsh"}),
            json!({"mode": "shell", "dialect": "cmd", "id": "", "label": "C", "path": "cmd"}),
            json!("auto"),
        ] {
            assert!(valid_shell(&shell).is_none(), "{shell}");
        }
        assert_eq!(
            valid_shell(&json!({"mode": "auto", "x": 1})),
            Some(json!({"mode": "auto"}))
        );
        assert_eq!(
            parse_params(&json!({"sessionId": "s", "scope": "user-execution"})),
            Some("user-execution")
        );
        assert!(parse_params(&json!({"sessionId": "s", "scope": "other"})).is_none());
        assert!(parse_params(&json!({"sessionId": "", "scope": "user-execution"})).is_none());
        assert!(
            parse_params(&json!({"sessionId": "s", "scope": "user-execution", "extra": 1}))
                .is_none()
        );
    }
}
