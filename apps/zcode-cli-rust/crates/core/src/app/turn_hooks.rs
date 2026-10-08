//! 会话级 hooks 调用点（TS runtime/methods/hooks.ts + turn.ts + turn-stop.ts，docs/specs/rust-hooks.md H1）：
//! SessionStart（本进程内每个会话一次）与 UserPromptSubmit 在首个模型请求之前，追加上下文以 system reminder
//! 插在本轮输入之前；UserPromptSubmit 阻止继续时撤回本轮输入、不请求模型。Stop 在纯文本收尾时运行，要求续跑时
//! 追加上下文并继续同一轮（至多 3 次）。
use super::context::{RunContext, TurnFacts};
use crate::contract::{Event, EventSink, ToolPort};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

const MAX_STOP_CONTINUATIONS: u32 = 3;
const CONTEXT_MAX_CHARS: usize = 24_000;
/// 本轮输入之前由 admission 注入的提醒（TS 在 hooks 之后才注入它们）。
const INPUT_REMINDERS: [&str; 3] = ["referenced_session_context", "date_change", "runtime_mode"];

/// 本轮输入（及其前置提醒）在消息里的起点；没有用户输入时为末尾。
pub(super) fn input_start(messages: &[Value]) -> usize {
    let Some(last) = messages
        .iter()
        .rposition(|m| m.get("_zcode_input").is_some())
    else {
        return messages.len();
    };
    let mut start = last;
    while start > 0
        && INPUT_REMINDERS
            .iter()
            .any(|s| messages[start - 1]["_zcode_source"] == *s)
    {
        start -= 1;
    }
    start
}

/// TS truncateForHook：按 UTF-16 码元截断并加省略号。
fn truncate(value: &str, max: usize) -> String {
    let units: Vec<u16> = value.encode_utf16().collect();
    if units.len() <= max {
        return value.to_owned();
    }
    format!("{}...", String::from_utf16_lossy(&units[..max]))
}

/// TS injectHookAdditionalContextIntoMessageHistory 的消息。
fn context_message(event: &str, contexts: &[String]) -> Option<Value> {
    if contexts.is_empty() {
        return None;
    }
    let body = contexts
        .iter()
        .enumerate()
        .map(|(i, c)| format!("#{}\n{c}", i + 1))
        .collect::<Vec<_>>()
        .join("\n\n");
    let body = truncate(
        &format!("{event} hook additional context: \n{body}"),
        CONTEXT_MAX_CHARS,
    );
    Some(
        json!({"role": "user", "content": crate::domain::plan_mode::wrap(&body), "_zcode_source": "hook_context"}),
    )
}

fn base(turn: &TurnFacts, event: &str) -> Value {
    json!({
        "hookEventName": event, "mode": turn.mode, "sessionId": turn.mcp_meta["session_id"],
        "traceId": turn.mcp_meta["trace_id"], "turnId": turn.mcp_meta["turn_id"],
    })
}

async fn run(tools: &dyn ToolPort, turn: &TurnFacts, input: Value) -> Option<Value> {
    let session = turn.mcp_meta["session_id"].as_str().unwrap_or_default();
    tools.run_hook(session, input, None).await
}

fn contexts(result: &Value) -> Vec<String> {
    result["additionalContexts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}

/// owner 提交消息变更后才继续（与历史一致落库）。
async fn commit(
    message: Option<Value>,
    before_input: bool,
    sink: &EventSink,
    cancel: &CancellationToken,
) -> Result<()> {
    let (committed, receipt) = oneshot::channel();
    sink.send(Event::HookContext {
        message,
        before_input,
        committed,
    })
    .await?;
    tokio::select! {biased;
        _ = cancel.cancelled() => bail!("Cancelled"),
        result = receipt => result.context("Session owner stopped before hook context commit"),
    }
}

/// SessionStart + UserPromptSubmit；返回 true 表示本轮被阻止（不请求模型）。
pub(super) async fn before_turn(
    tools: &dyn ToolPort,
    model: Option<String>,
    history: &mut RunContext,
    sink: &EventSink,
    cancel: &CancellationToken,
) -> Result<bool> {
    let turn = &history.turn;
    let mut injected = vec![];
    if let Some(source) = turn.session_start {
        let mut input = base(turn, "SessionStart");
        input["source"] = source.into();
        if let Some(model) = model {
            input["model"] = model.into();
        }
        if let Some(result) = run(tools, turn, input).await {
            injected.extend(context_message("SessionStart", &contexts(&result)));
        }
    }
    let mut blocked = false;
    if let Some(prompt) = turn.prompt.clone() {
        let mut input = base(turn, "UserPromptSubmit");
        input["prompt"] = prompt.into();
        if let Some(result) = run(tools, turn, input).await {
            blocked = result["preventContinuation"] == true;
            if !blocked {
                injected.extend(context_message("UserPromptSubmit", &contexts(&result)));
            }
        }
    }
    for message in injected {
        commit(Some(message.clone()), true, sink, cancel).await?;
        history.insert(input_start(&history.messages), message);
    }
    if blocked {
        commit(None, true, sink, cancel).await?;
        let start = input_start(&history.messages);
        history.messages.truncate(start);
    }
    Ok(blocked)
}

/// Stop：返回 true 表示 hook 要求续跑（上下文已追加进历史）。
pub(super) async fn stop(
    tools: &dyn ToolPort,
    turn: &TurnFacts,
    history: &mut RunContext,
    count: &mut u32,
    (response, tool_calls): (&str, usize),
    sink: &EventSink,
    cancel: &CancellationToken,
) -> Result<bool> {
    let mut input = base(turn, "Stop");
    input["responseText"] = response.into();
    input["stopHookActive"] = (*count > 0).into();
    input["toolCallCount"] = tool_calls.into();
    let Some(result) = run(tools, turn, input).await else {
        return Ok(false);
    };
    let contexts = contexts(&result);
    if result["stopShouldContinue"] != true
        || contexts.is_empty()
        || *count >= MAX_STOP_CONTINUATIONS
    {
        return Ok(false);
    }
    *count += 1;
    let message = context_message("Stop", &contexts).unwrap();
    commit(Some(message.clone()), false, sink, cancel).await?;
    history.push(message);
    Ok(true)
}

impl super::Engine {
    /// `Event::HookContext`：hook 上下文落进会话消息（`before_input` 时插在本轮输入之前）；`None` 撤回本轮输入。
    pub(super) async fn hook_context(
        &mut self,
        id: &str,
        message: Option<Value>,
        before_input: bool,
        committed: oneshot::Sender<()>,
    ) -> Result<()> {
        let Some(s) = self.sessions.get_mut(id) else {
            return Ok(());
        };
        let start = input_start(&s.messages).max(s.context.offset);
        // 插入 / 撤回改动了已落库的消息区段：整段重写（存储按追加增量写）。
        match message {
            Some(message) if before_input => {
                if let Some(tokens) = &mut s.context_tokens {
                    *tokens += crate::domain::context::estimate(std::slice::from_ref(&message));
                }
                s.messages.insert(start, message);
                if let Some((at, _)) = s.shell_notice.as_mut().filter(|(at, _)| start < *at) {
                    *at += 1;
                }
                s.history_rewrite = true;
            }
            Some(message) => s.append_message(message),
            None => {
                s.messages.truncate(start);
                if s.shell_notice.as_ref().is_some_and(|(at, _)| *at > start) {
                    s.shell_notice = None;
                }
                s.context_tokens = None;
                s.history_rewrite = true;
            }
        }
        self.persist(id, None).await?;
        let _ = committed.send(());
        Ok(())
    }
}
