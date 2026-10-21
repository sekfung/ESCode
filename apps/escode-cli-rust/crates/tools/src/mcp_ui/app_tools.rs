//! App-Provided Tools 的登记表与模型工具面（docs/specs/rust-mcp-ui.md §4 P3）。
//!
//! 登记表是按会话的 live-only 状态：同 server 只暴露最近登记的实例；模型调用经"信箱"
//! （`pluginUi.appToolCall` live delta）投递给唯一实例，页面 claim 后 `tools/call`，
//! 结果经 `mcp/uiResolveAppToolCall` 回传。认领 5 秒、执行 30 秒、结果 ≤10 MiB。
use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::contract::ToolOutput;

pub(crate) const CLAIM_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const EXECUTION_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_PER_INSTANCE: usize = 50;
const ARGUMENTS_MAX_BYTES: usize = 1024 * 1024;
const RESULT_MAX_BYTES: usize = 10 * 1024 * 1024;

struct InstanceRecord {
    plugin_id: String,
    server_name: String,
    scope_id: String,
    generation: u64,
    seq: u64,
    tools: Vec<Value>,
    /// 完整实例凭证（delta 的 subscribers 需要）。
    credential: Value,
}

struct PendingCall {
    instance_key: String,
    claimed: bool,
    claim: Option<oneshot::Sender<()>>,
    settle: Option<oneshot::Sender<std::result::Result<Value, String>>>,
}

#[derive(Default)]
struct SessionState {
    instances: BTreeMap<String, InstanceRecord>,
    next_seq: u64,
    pending: BTreeMap<String, PendingCall>,
}

/// `(callId, 信箱 delta, 认领信号, 结果回传)`。
pub(crate) type PreparedCall = (
    String,
    Value,
    oneshot::Receiver<()>,
    oneshot::Receiver<std::result::Result<Value, String>>,
);

#[derive(Default)]
pub(crate) struct AppTools {
    sessions: BTreeMap<String, SessionState>,
}

fn instance_key(scope_id: &str, generation: u64) -> String {
    format!("{scope_id}|{generation}")
}

fn model_name(server_name: &str, tool_name: &str) -> String {
    format!("app__{}__{}", sanitize(server_name), sanitize(tool_name))
}

/// 与 MCP 工具名同一清洗规则：非 `[A-Za-z0-9_-]` 替换为 `_` 并合并连续下划线。
fn sanitize(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') {
            if ch == '_' && result.ends_with('_') {
                continue;
            }
            result.push(ch);
        } else if !result.ends_with('_') {
            result.push('_');
        }
    }
    if result.is_empty() {
        "unknown".to_owned()
    } else {
        result
    }
}

impl AppTools {
    /// 登记/替换实例；返回该实例暴露给模型的工具名。
    pub(crate) fn register(&mut self, session: &str, params: &Value) -> Result<Value> {
        let scope_id = text(params, "scopeId")?;
        let generation = params["generation"]
            .as_u64()
            .context("generation is required")?;
        let server_name = text(params, "serverName")?;
        let plugin_id = text(params, "pluginId")?;
        let credential = params
            .get("instance")
            .cloned()
            .context("instance is required")?;
        let tools = params["tools"]
            .as_array()
            .cloned()
            .context("tools are required")?;
        if tools.len() > MAX_PER_INSTANCE {
            bail!("Too many App tools");
        }
        let key = instance_key(&scope_id, generation);
        let state = self.sessions.entry(session.to_owned()).or_default();
        // 同沙箱作用域的新代际替换旧代际。
        let stale: Vec<String> = state
            .instances
            .iter()
            .filter(|(existing, record)| *existing != &key && record.scope_id == scope_id)
            .map(|(existing, _)| existing.clone())
            .collect();
        for stale in stale {
            drop_record(state, &stale);
        }
        let seq = match state.instances.get(&key) {
            Some(record) => record.seq,
            None => {
                state.next_seq += 1;
                state.next_seq
            }
        };
        state.instances.insert(
            key.clone(),
            InstanceRecord {
                plugin_id,
                server_name,
                scope_id,
                generation,
                seq,
                tools,
                credential,
            },
        );
        let exposed = exposed(state);
        let mine = exposed
            .iter()
            .filter(|(model, record, _)| {
                record.scope_id == state.instances[&key].scope_id
                    && record.generation == state.instances[&key].generation
                    && record.seq == state.instances[&key].seq
                    && model.starts_with(&format!(
                        "app__{}__",
                        sanitize(&state.instances[&key].server_name)
                    ))
            })
            .map(|(model, _, tool)| {
                json!({
                    "name": tool["name"],
                    "modelName": model,
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({ "tools": mine }))
    }

    pub(crate) fn unregister(&mut self, session: &str, params: &Value) -> usize {
        let (Ok(scope_id), Some(generation)) =
            (text(params, "scopeId"), params["generation"].as_u64())
        else {
            return 0;
        };
        let Some(state) = self.sessions.get_mut(session) else {
            return 0;
        };
        let key = instance_key(&scope_id, generation);
        let removed = state
            .instances
            .get(&key)
            .map_or(0, |record| record.tools.len());
        drop_record(state, &key);
        removed
    }

    pub(crate) fn claim(&mut self, session: &str, params: &Value) -> bool {
        let (Ok(scope_id), Some(generation), Ok(call_id)) = (
            text(params, "scopeId"),
            params["generation"].as_u64(),
            text(params, "callId"),
        ) else {
            return false;
        };
        let key = instance_key(&scope_id, generation);
        let Some(state) = self.sessions.get_mut(session) else {
            return false;
        };
        let Some(pending) = state.pending.get_mut(&call_id) else {
            return false;
        };
        if pending.instance_key != key {
            return false;
        }
        if pending.claimed {
            return true;
        }
        pending.claimed = true;
        if let Some(claim) = pending.claim.take() {
            let _ = claim.send(());
        }
        true
    }

    pub(crate) fn resolve(&mut self, session: &str, params: &Value) -> bool {
        let (Ok(scope_id), Some(generation), Ok(call_id)) = (
            text(params, "scopeId"),
            params["generation"].as_u64(),
            text(params, "callId"),
        ) else {
            return false;
        };
        let key = instance_key(&scope_id, generation);
        let Some(state) = self.sessions.get_mut(session) else {
            return false;
        };
        let Some(pending) = state.pending.get_mut(&call_id) else {
            return false;
        };
        if !pending.claimed || pending.instance_key != key {
            return false;
        }
        let settle = pending.settle.take();
        let outcome = if let Some(error) = params.get("error") {
            Err(error["message"]
                .as_str()
                .filter(|message| !message.is_empty())
                .unwrap_or("App tool failed")
                .to_owned())
        } else {
            let result = params.get("result").cloned().unwrap_or(Value::Null);
            if result.to_string().len() > RESULT_MAX_BYTES {
                Err(format!("App tool result exceeds {RESULT_MAX_BYTES} bytes"))
            } else {
                Ok(result)
            }
        };
        if let Some(settle) = settle {
            let _ = settle.send(outcome);
        }
        true
    }

    /// 模型工具面：同 server 只暴露最近实例，按 server 名排序并去重。
    pub(crate) fn definitions(&self, session: &str) -> Vec<Value> {
        let Some(state) = self.sessions.get(session) else {
            return Vec::new();
        };
        exposed(state)
            .into_iter()
            .map(|(model, _, tool)| {
                json!({"type":"function","function":{
                    "name": model,
                    "description": tool["description"].as_str().unwrap_or(""),
                    "parameters": tool["inputSchema"].clone(),
                }})
            })
            .collect()
    }

    /// 取出一次模型调用：返回 `(callId, delta, claim, settle)`；登记待认领调用。
    pub(crate) fn prepare_call(
        &mut self,
        session: &str,
        tool_name: &str,
        arguments: &Value,
    ) -> Result<PreparedCall> {
        if arguments.to_string().len() > ARGUMENTS_MAX_BYTES {
            bail!("App tool arguments are too large");
        }
        let state = self
            .sessions
            .get_mut(session)
            .context("Session unavailable")?;
        let (_, record, tool) = exposed(state)
            .into_iter()
            .find(|(model, _, _)| model == tool_name)
            .context("App tool is not running")?;
        let call_id = uuid::Uuid::new_v4().to_string();
        let instance = instance_key(&record.scope_id, record.generation);
        let plugin_id = record.plugin_id.clone();
        let server_name = record.server_name.clone();
        let tool_name = tool["name"].as_str().unwrap_or_default().to_owned();
        let subscribers = vec![json!({
            "scopeId": record.scope_id,
            "generation": record.generation,
            "instance": record.credential,
        })];
        let (claim_tx, claim_rx) = oneshot::channel();
        let (settle_tx, settle_rx) = oneshot::channel();
        state.pending.insert(
            call_id.clone(),
            PendingCall {
                instance_key: instance,
                claimed: false,
                claim: Some(claim_tx),
                settle: Some(settle_tx),
            },
        );
        let delta = json!({
            "op": "pluginUi.appToolCall",
            "pluginId": plugin_id,
            "serverName": server_name,
            "subscribers": subscribers,
            "callId": call_id,
            "toolName": tool_name,
            "arguments": arguments,
        });
        Ok((call_id, delta, claim_rx, settle_rx))
    }

    pub(crate) fn finish_call(&mut self, session: &str, call_id: &str) {
        if let Some(state) = self.sessions.get_mut(session) {
            state.pending.remove(call_id);
        }
    }

    pub(crate) fn remove_instance(&mut self, session: &str, scope_id: &str, generation: u64) {
        if let Some(state) = self.sessions.get_mut(session) {
            drop_record(state, &instance_key(scope_id, generation));
        }
    }

    pub(crate) fn clear_session(&mut self, session: &str) {
        if let Some(state) = self.sessions.remove(session) {
            for (_, pending) in state.pending {
                if let Some(settle) = pending.settle {
                    let _ = settle.send(Err("Session was closed".to_owned()));
                }
            }
        }
    }
}

fn drop_record(state: &mut SessionState, key: &str) {
    if state.instances.remove(key).is_none() {
        return;
    }
    let call_ids: Vec<String> = state
        .pending
        .iter()
        .filter(|(_, pending)| pending.instance_key == key)
        .map(|(id, _)| id.clone())
        .collect();
    for id in call_ids {
        if let Some(pending) = state.pending.remove(&id)
            && let Some(settle) = pending.settle
        {
            let _ = settle.send(Err("App page was closed".to_owned()));
        }
    }
}

fn exposed(state: &SessionState) -> Vec<(String, &InstanceRecord, &Value)> {
    let mut latest: BTreeMap<&str, &InstanceRecord> = BTreeMap::new();
    for record in state.instances.values() {
        let replace = latest
            .get(record.server_name.as_str())
            .is_none_or(|current| record.seq > current.seq);
        if replace {
            latest.insert(record.server_name.as_str(), record);
        }
    }
    let mut result = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for record in latest.values() {
        for tool in &record.tools {
            let name = model_name(
                &record.server_name,
                tool["name"].as_str().unwrap_or_default(),
            );
            if seen.insert(name.clone()) {
                result.push((name, *record, tool));
            }
        }
    }
    result
}

fn text(params: &Value, key: &'static str) -> Result<String> {
    params[key]
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .with_context(|| format!("{key} is required"))
}

// 需要导入到调用方（model 调用超时常量在 Hub 的执行函数里使用）。
pub(crate) use CLAIM_TIMEOUT as APP_TOOL_CLAIM_TIMEOUT;

/// 模型调用等待认领与执行；返回页面回传的 MCP 结果。
pub(crate) async fn await_result(
    claim: oneshot::Receiver<()>,
    settle: oneshot::Receiver<std::result::Result<Value, String>>,
    server_name: &str,
    cancel: &CancellationToken,
) -> Result<Value> {
    tokio::select! {biased;
        _=cancel.cancelled()=>bail!("App tool call was cancelled"),
        result=tokio::time::timeout(APP_TOOL_CLAIM_TIMEOUT, claim)=>{
            if !matches!(result, Ok(Ok(()))) {
                bail!("The {server_name} app page is not running");
            }
        }
    }
    tokio::select! {biased;
        _=cancel.cancelled()=>bail!("App tool call was cancelled"),
        result=tokio::time::timeout(EXECUTION_TIMEOUT, settle)=>match result {
            Ok(Ok(Ok(value)))=>Ok(value),
            Ok(Ok(Err(message)))=>bail!("{message}"),
            _=>bail!("App tool call timed out"),
        }
    }
}

/// 页面结果 → 工具输出（保留原始 MCP 形状，不生成对话行）。
pub(crate) fn to_output(result: Value) -> ToolOutput {
    let failed = result["isError"] == true;
    let mut output = ToolOutput::text(serde_json::to_string(&result).unwrap_or_default());
    output.failed = failed;
    output.data = result;
    output
}

#[cfg(test)]
#[path = "app_tools_tests.rs"]
mod tests;
