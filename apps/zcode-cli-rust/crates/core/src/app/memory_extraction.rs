//! 项目记忆自动提取（docs/specs/rust-project-memory.md）：每会话一个调度（Engine 持有），串行执行、
//! 运行中只保留最新待处理快照；判定在处理时按当时游标进行；受限 agent loop 最多 5 轮，
//! 工具按 TS evaluateMemoryAgentToolPolicy 收窄，不写会话、不发布行。模型鉴权借用 auxiliary 作业通道。
use super::Engine;
use crate::contract::{Event, EventSink, MemorySnapshot, ToolPort};
use crate::domain::memory::{self, Decision, DecisionMessage, ToolKind};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

struct Pending {
    snapshot: MemorySnapshot,
    decisions: Vec<DecisionMessage>,
    boundary: usize,
}
#[derive(Default)]
pub(super) struct MemoryScheduler {
    cursor: Option<usize>,
    running: bool,
    pending: Option<Pending>,
    shutdown: CancellationToken,
}
pub(super) type SharedScheduler = Arc<Mutex<MemoryScheduler>>;

/// 会话消息 → 判定摘要。工具结果在 TS 中属于 assistant 消息的 part，这里不单独计数。
fn decision_messages(messages: &[Value]) -> Vec<DecisionMessage> {
    messages
        .iter()
        .filter(|m| m["role"] != "tool")
        .map(|m| match m["role"].as_str() {
            Some("user") => match m.get("_zcode_input") {
                Some(prose) => DecisionMessage::User {
                    prose: prose == true,
                },
                None => DecisionMessage::Other,
            },
            Some("assistant") => DecisionMessage::Assistant {
                writes: m["tool_calls"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|c| matches!(c["function"]["name"].as_str(), Some("Write" | "Edit")))
                    .filter_map(|c| {
                        serde_json::from_str::<Value>(c["function"]["arguments"].as_str()?)
                            .ok()?["file_path"]
                            .as_str()
                            .map(str::to_owned)
                    })
                    .collect(),
            },
            _ => DecisionMessage::Other,
        })
        .collect()
}

impl Engine {
    pub(super) fn schedule_memory(&mut self, id: &str, snapshot: MemorySnapshot) {
        let Some(session) = self.sessions.get(id) else {
            return;
        };
        let decisions = decision_messages(&session.messages);
        let Some(boundary) = decisions.len().checked_sub(1) else {
            return;
        };
        let state = self
            .memory_schedulers
            .entry(id.to_owned())
            .or_default()
            .clone();
        let mut guard = state.lock().unwrap();
        if guard.shutdown.is_cancelled() {
            return;
        }
        guard.pending = Some(Pending {
            snapshot,
            decisions,
            boundary,
        });
        if guard.running {
            return;
        }
        guard.running = true;
        let cancel = guard.shutdown.clone();
        drop(guard);
        let job = format!("memory-extract:{}", self.clock.id());
        self.auxiliary.insert(
            job.clone(),
            super::auxiliary::Auxiliary {
                request: None,
                cancel: cancel.clone(),
                operation: None,
                session: Some(id.to_owned()),
            },
        );
        let sink = EventSink {
            session_id: job.clone(),
            run_id: job,
            tx: self.events.clone(),
        };
        let tools = self.tools.clone();
        let cwd = self.workspace_path.clone();
        let origin = id.to_owned();
        tokio::spawn(async move {
            drain(&state, tools.as_ref(), &cwd, &origin, &sink, &cancel).await;
            let _ = tools.close_session(&sink.session_id).await;
            let _ = sink
                .send(Event::AuxiliaryDone {
                    result: Ok(Value::Null),
                })
                .await;
        });
    }
    /// 关闭会话时取消进行中的提取并丢弃待处理快照（TS scheduler.shutdown）。
    pub(super) fn shutdown_memory(&mut self, id: &str) {
        if let Some(state) = self.memory_schedulers.remove(id) {
            let mut guard = state.lock().unwrap();
            guard.pending = None;
            guard.shutdown.cancel();
        }
        self.memory_prompts.remove(id);
    }
}

async fn drain(
    state: &SharedScheduler,
    tools: &dyn ToolPort,
    cwd: &str,
    origin: &str,
    sink: &EventSink,
    cancel: &CancellationToken,
) {
    loop {
        let (pending, cursor) = {
            let mut guard = state.lock().unwrap();
            match guard.pending.take().filter(|_| !cancel.is_cancelled()) {
                Some(pending) => (pending, guard.cursor),
                None => {
                    guard.running = false;
                    return;
                }
            }
        };
        let root = pending.snapshot.memory.root.clone();
        let advance = match memory::decide(&pending.decisions, cursor, &root, cwd) {
            Decision::Skip => true,
            Decision::Run(count) => {
                // 记忆 agent 继承主会话读取状态，写入的记忆文件以主会话为来源。
                tools
                    .memory_context(&sink.session_id, &root, origin, Some(origin), None)
                    .await;
                extract(&pending.snapshot, count, tools, cwd, sink, cancel)
                    .await
                    .is_ok()
            }
        };
        if advance && !cancel.is_cancelled() {
            state.lock().unwrap().cursor = Some(pending.boundary);
        }
    }
}

async fn extract(
    snapshot: &MemorySnapshot,
    count: usize,
    tools: &dyn ToolPort,
    cwd: &str,
    sink: &EventSink,
    cancel: &CancellationToken,
) -> anyhow::Result<()> {
    let root = &snapshot.memory.root;
    let manifest = tools.memory_manifest(root).await;
    let mut messages = snapshot.messages.clone();
    messages.push(json!({"role":"user","content":memory::extraction_prompt(&manifest, count)}));
    // TS auxiliaryModelOptions：最低推理档位，输出上限 min(5000, max)。
    let model = snapshot
        .model
        .auxiliary()
        .unwrap_or_else(|| snapshot.model.clone());
    let model = model.with_max_output_tokens(5_000)?.unwrap_or(model);
    for _ in 0..memory::EXTRACTION_MAX_TURNS {
        let output = model
            .complete(messages.clone(), &snapshot.definitions, sink, cancel)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        messages.push(output.message.clone());
        if output.calls.is_empty() {
            break;
        }
        let results = futures_util::future::join_all(
            output
                .calls
                .iter()
                .map(|call| run_tool(call, &snapshot.definitions, root, cwd, tools, sink, cancel)),
        )
        .await;
        for (call, (content, failed)) in output.calls.iter().zip(results) {
            messages.push(json!({"role":"tool","tool_call_id":call["id"],"content":content,"_zcode_tool_failed":failed}));
        }
    }
    Ok(())
}

async fn run_tool(
    call: &Value,
    definitions: &[Value],
    root: &str,
    cwd: &str,
    tools: &dyn ToolPort,
    sink: &EventSink,
    cancel: &CancellationToken,
) -> (String, bool) {
    let name = call["function"]["name"].as_str().unwrap_or("");
    let input: Value = call["function"]["arguments"]
        .as_str()
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_else(|| json!({}));
    let kind = if !definitions.iter().any(|d| d["function"]["name"] == name) {
        ToolKind::Missing
    } else if tools
        .permission_capability(name, &input)
        .and_then(|c| c.side_effect_scope)
        .as_deref()
        == Some("network")
    {
        ToolKind::Network
    } else {
        ToolKind::Local
    };
    if let Err(reason) =
        memory::tool_policy(name, &input, kind, root, cwd, &|c| tools.readonly_bash(c))
    {
        return (reason, true);
    }
    match tools.execute_scoped(name, &input, sink, cancel).await {
        Ok(output) => (output.content, output.failed),
        Err(error) => (error.to_string(), true),
    }
}
