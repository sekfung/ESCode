//! `mcp/uiSampling` 的账本与模型调用（docs/specs/rust-mcp-ui.md §4 P4）。
//!
//! 账本是采样接纳/取消/终态的唯一 owner：先 pin 后执行，同代 operationId 不重放；
//! 每实例同时最多 1 个操作，会话 4、Agent 8，60 秒绝对期限。
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::Engine;
use crate::contract::{EventSink, ModelIdentity, ModelPort};

pub(super) const TIMEOUT: Duration = Duration::from_secs(60);
const MAX_PER_SESSION: usize = 4;
const MAX_PER_AGENT: usize = 8;
const MAX_OPERATIONS: usize = 4096;

pub(super) struct Scope {
    pub(super) token: String,
    pub(super) session: String,
}

#[derive(Default)]
pub(super) struct Calls {
    /// 实例 token → operationId → 进行中的取消令牌（`None` = 已终结，防重放）。
    ledgers: BTreeMap<String, BTreeMap<String, Option<CancellationToken>>>,
    sessions: BTreeMap<String, usize>,
    active: usize,
}

impl Calls {
    pub(super) fn begin(&mut self, scope: &Scope, operation: &str) -> Result<CancellationToken> {
        let ledger = self.ledgers.entry(scope.token.clone()).or_default();
        if ledger.len() >= MAX_OPERATIONS {
            bail!("Sampling operation limit reached; reopen the App");
        }
        if ledger.contains_key(operation) {
            bail!("Duplicate sampling operationId");
        }
        if self.active >= MAX_PER_AGENT
            || self.sessions.get(&scope.session).copied().unwrap_or(0) >= MAX_PER_SESSION
            || ledger.values().any(Option::is_some)
        {
            bail!("Sampling busy");
        }
        let cancel = CancellationToken::new();
        ledger.insert(operation.to_owned(), Some(cancel.clone()));
        self.active += 1;
        *self.sessions.entry(scope.session.clone()).or_default() += 1;
        Ok(cancel)
    }

    pub(super) fn finish(&mut self, scope: &Scope, operation: &str) {
        if let Some(slot) = self
            .ledgers
            .get_mut(&scope.token)
            .and_then(|ledger| ledger.get_mut(operation))
        {
            *slot = None;
        }
        self.active = self.active.saturating_sub(1);
        if let Some(count) = self.sessions.get_mut(&scope.session) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.sessions.remove(&scope.session);
            }
        }
    }

    /// 取消先于 sample 到达时也留下终态；旧 operationId 之后不会被执行。
    pub(super) fn cancel(&mut self, scope: &Scope, operation: &str) -> bool {
        let ledger = self.ledgers.entry(scope.token.clone()).or_default();
        if ledger.contains_key(operation) {
            let controller = ledger.insert(operation.to_owned(), None).flatten();
            if let Some(controller) = &controller {
                controller.cancel();
            }
            return controller.is_some();
        }
        ledger.insert(operation.to_owned(), None);
        false
    }
}

impl Engine {
    pub(super) async fn sample_mcp_ui(&mut self, params: &Value) -> Result<Value> {
        let scope = sampling_scope(params)?;
        let operation = params["operationId"]
            .as_str()
            .filter(|value| !value.is_empty())
            .context("operationId is required")?
            .to_owned();
        self.tools.mcp_ui_begin(params)?;
        let cancel = match self.sampling.begin(&scope, &operation) {
            Ok(cancel) => cancel,
            Err(error) => {
                self.tools.mcp_ui_end(params);
                return Err(error);
            }
        };
        let request = params["request"].clone();
        let result = tokio::time::timeout(
            TIMEOUT,
            run_sampling(&self.model, &self.config, &request, &cancel),
        )
        .await;
        self.tools.mcp_ui_end(params);
        self.sampling.finish(&scope, &operation);
        match result {
            Ok(result) => {
                // 迟到结果不投递给已失效实例。
                self.tools.mcp_ui_validate_scope(params)?;
                result
            }
            Err(_) => bail!("Sampling timed out"),
        }
    }

    pub(super) fn cancel_mcp_sampling(&mut self, params: &Value) -> Result<Value> {
        let scope = sampling_scope(params)?;
        let operation = params["operationId"]
            .as_str()
            .filter(|value| !value.is_empty())
            .context("operationId is required")?;
        Ok(json!({ "cancelled": self.sampling.cancel(&scope, operation) }))
    }

    /// server → client sampling（`sampling/createMessage`）：无页面实例，直接借任务模型应答。
    pub(super) async fn sample_from_server(&mut self, request: &Value) -> Result<Value> {
        let cancel = CancellationToken::new();
        run_sampling(&self.model, &self.config, request, &cancel).await
    }
}

fn sampling_scope(params: &Value) -> Result<Scope> {
    let token = params
        .get("instance")
        .and_then(|instance| instance["token"].as_str())
        .filter(|value| !value.is_empty())
        .context("instance.token is required")?;
    let session = params["sessionId"]
        .as_str()
        .filter(|value| !value.is_empty())
        .context("sessionId is required")?;
    Ok(Scope {
        token: token.to_owned(),
        session: session.to_owned(),
    })
}

async fn run_sampling(
    model: &Option<Arc<dyn ModelPort>>,
    config: &Option<ModelIdentity>,
    request: &Value,
    cancel: &CancellationToken,
) -> Result<Value> {
    let model = model.clone().context("Model unavailable")?;
    let messages = sampling_messages(request)?;
    let max_tokens = request["maxTokens"].as_u64().unwrap_or(1024) as usize;
    let model = model.with_max_output_tokens(max_tokens)?.unwrap_or(model);
    // 采样不进入会话投影：事件只排空，不转发。
    let (tx, mut rx) = mpsc::channel(32);
    let sink = EventSink {
        session_id: "mcp-sampling".into(),
        run_id: "mcp-sampling".into(),
        tx,
    };
    let complete = model.complete(messages, &[], &sink, cancel);
    tokio::pin!(complete);
    let output = loop {
        tokio::select! {biased;
            _=cancel.cancelled()=>bail!("Sampling cancelled"),
            Some(_)=rx.recv()=>{},
            result=&mut complete=>break result?,
        }
    };
    let text = assistant_text(&output.message)?;
    let model_name = config
        .as_ref()
        .map(|config| config.model_id.clone())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "unknown".to_owned());
    Ok(json!({
        "role": "assistant",
        "model": model_name,
        "content": { "type": "text", "text": text },
        "stopReason": if output.output_limit { "maxTokens" } else { "endTurn" },
    }))
}

/// MCP sampling 的 text-only 子集；图片等块显式拒绝，不静默丢弃。
fn sampling_messages(request: &Value) -> Result<Vec<Value>> {
    let mut messages = vec![];
    if let Some(system) = request["systemPrompt"]
        .as_str()
        .filter(|value| !value.is_empty())
    {
        messages.push(json!({ "role": "system", "content": system }));
    }
    for message in request["messages"]
        .as_array()
        .context("sampling messages are required")?
    {
        let role = message["role"]
            .as_str()
            .context("message role is required")?;
        let content = &message["content"];
        let text = match content {
            Value::String(text) => text.clone(),
            Value::Object(block) => block_text(block)?,
            Value::Array(blocks) => blocks
                .iter()
                .map(|block| {
                    block
                        .as_object()
                        .context("unsupported sampling content")
                        .and_then(block_text)
                })
                .collect::<Result<Vec<_>>>()?
                .join("\n"),
            _ => bail!("Unsupported sampling content"),
        };
        messages.push(json!({ "role": role, "content": text }));
    }
    Ok(messages)
}

fn block_text(block: &serde_json::Map<String, Value>) -> Result<String> {
    if block.get("type").and_then(Value::as_str) != Some("text") {
        bail!("Unsupported sampling content");
    }
    block["text"]
        .as_str()
        .map(str::to_owned)
        .context("sampling text block is required")
}

fn assistant_text(message: &Value) -> Result<String> {
    match &message["content"] {
        Value::String(text) => Ok(text.clone()),
        Value::Array(blocks) => Ok(blocks
            .iter()
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("")),
        _ => bail!("Model returned no text for sampling"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(token: &str, session: &str) -> Scope {
        Scope {
            token: token.into(),
            session: session.into(),
        }
    }

    #[test]
    fn sampling_ledger_rejects_duplicates_and_busy_instances() {
        let mut calls = Calls::default();
        let scope = scope("token", "session");
        let cancel = calls.begin(&scope, "op-1").expect("first");
        assert!(
            calls.begin(&scope, "op-1").is_err(),
            "重复 operationId 必须拒绝"
        );
        assert!(
            calls.begin(&scope, "op-2").is_err(),
            "同实例同时只允许 1 个操作"
        );
        calls.finish(&scope, "op-1");
        assert!(!cancel.is_cancelled());
        assert!(
            calls.begin(&scope, "op-1").is_err(),
            "终态 operationId 不可重放"
        );
        assert!(calls.begin(&scope, "op-2").is_ok());
    }

    #[test]
    fn cancel_before_sample_leaves_a_terminal_entry() {
        let mut calls = Calls::default();
        let scope = scope("token", "session");
        assert!(!calls.cancel(&scope, "op-1"));
        assert!(
            calls.begin(&scope, "op-1").is_err(),
            "取消先到时旧 operationId 不执行"
        );
        assert!(calls.begin(&scope, "op-2").is_ok());
        assert!(calls.cancel(&scope, "op-2"));
        assert!(!calls.cancel(&scope, "op-2"), "取消幂等");
    }

    #[test]
    fn session_limit_is_enforced() {
        let mut calls = Calls::default();
        for index in 0..MAX_PER_SESSION {
            let scoped = scope(&format!("token-{index}"), "session");
            calls.begin(&scoped, "op").expect("within limit");
        }
        assert!(
            calls.begin(&scope("token-extra", "session"), "op").is_err(),
            "同一会话超过并发上限必须拒绝"
        );
        let other = scope("token-other", "other-session");
        assert!(calls.begin(&other, "op").is_ok());
    }
}
