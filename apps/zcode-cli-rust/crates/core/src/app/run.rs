use super::Engine;
use crate::contract::{Event, EventSink as Sink, ModelPort};
use anyhow::{Context, Result};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

impl Engine {
    pub(super) fn start_run(&mut self, id: &str, turn_id: String) -> Result<()> {
        self.telemetry_turn_started(id, &turn_id);
        let turn_id_for_facts = turn_id.clone();
        let payload = self
            .sessions
            .get(id)
            .and_then(|s| s.history.inputs.iter().rev().find(|i| i.turn == turn_id).map(|i| i.payload.clone()))
            .unwrap_or_default();
        // 执行作用域：本轮模型取输入的 modelSelection，不读写会话选择（docs/specs/rust-offpeak.md 第三期）。
        let identity = if payload.get(super::model_execution::MARKER).is_some() {
            self.select(&payload, Some(self.session_selection(id)?))?
        } else {
            self.session_selection(id)?
        };
        let (selection, updates) = tokio::sync::watch::channel(identity.clone());
        let model: Arc<dyn ModelPort> = if let Some(registry) = &self.registry {
            registry.resolve(&identity)?;
            Arc::new(super::model_config::LiveModel {
                registry: registry.clone(),
                selection: updates,
            })
        } else {
            self.model.clone().context("Model configuration required")?
        };
        // `/goal` 的标题 sidecar 不等主轮次（TS control-only turn 边界即启动）。
        self.start_goal_title(id);
        let session = self.sessions.get_mut(id).context("Session unavailable")?;
        let estimated = session.active_context_tokens();
        let run_id = session.run_id.clone().context("Run reservation required")?;
        self.begin_execution(id, &run_id, &turn_id_for_facts, &payload, &identity);
        let session = self.sessions.get_mut(id).context("Session unavailable")?;
        let cancel = CancellationToken::new();
        self.active.insert(
            id.into(),
            Active {
                selection,
                cancel: cancel.clone(),
                run_id: run_id.clone(),
                turn_id,
                response_id: None,
            },
        );
        let manual = session
            .rows
            .last()
            .filter(|r| r["kind"] == "turnHeader" && r["executionKind"] == "controlOnly")
            .map(|_| session.compact_instructions.clone().unwrap_or_default());
        let mut history = super::context::RunContext::new(
            session.context.clone(),
            session.messages[session.context.offset..].to_vec(),
            manual,
            estimated,
        );
        history.prompt_snapshot = session.prompt_snapshot.clone();
        history.subagents_enabled = self.subagents_enabled;
        history.skills = session.skills.clone();
        history.goal = session.goal.clone();
        history.agent_profile = session.agent_profile.clone();
        history.workflow_actor = session.workflow_actor.clone();
        let input = session
            .history
            .inputs
            .iter()
            .rev()
            .find(|i| i.turn == turn_id_for_facts)
            .map(|i| i.payload.clone())
            .unwrap_or_default();
        history.turn = super::context::TurnFacts {
            automation_id: input["automationId"].as_str().map(str::to_owned),
            disallowed: input["toolDisallowlist"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|t| t.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            bot_delivery_target: Some(input["botDeliveryTarget"].clone()).filter(|v| v.is_object()),
            mode: session.mode.clone(),
            mcp_meta: {
                let client = self
                    .subscriptions
                    .values()
                    .rev()
                    .find(|s| s.topic == format!("conversation/{id}"))
                    .map(|s| s.client_mode.clone())
                    .unwrap_or_else(|| "desktop-continuous".into());
                let mut meta = serde_json::json!({
                    "trace_id": session.trace_id.clone().unwrap_or_else(|| self.clock.id()),
                    "parent_span_id": self.clock.id().chars().take(16).collect::<String>(),
                    "session_id": id,
                    "turn_id": turn_id_for_facts,
                    "runtime_scope": if session.task_type == "subagent_child" { "subagent" } else { "main" },
                    "workspace_path": self.workspace_path,
                    "workspace_key": self.workspace,
                    "client_mode": client,
                    "delivery_kind": client,
                });
                if self.workspace != self.workspace_path {
                    meta["workspace_identity"] = self.workspace.clone().into();
                }
                meta
            },
            model_selection: Some(if session.reasoning_level.is_empty() {
                serde_json::json!({"providerId": session.provider, "modelId": session.model})
            } else {
                serde_json::json!({"providerId": session.provider, "modelId": session.model, "options": {"reasoningLevel": session.reasoning_level}})
            }),
            ..Default::default()
        };
        // 闲时派发事实（docs/specs/rust-offpeak.md 第二期）：admission 固化的闲时身份；禁用列表含 OffPeakCreate 视为受限轮。
        history.turn.off_peak_task_id = input["offPeakTaskId"].as_str().map(str::to_owned);
        history.turn.off_peak_restricted = history.turn.off_peak_task_id.is_some()
            || history.turn.disallowed.iter().any(|t| t == "OffPeakCreate");
        history.turn.off_peak_tools = self.off_peak_enabled(id);
        // 动态工作流灰度门按会话固化值读（docs/specs/rust-dynamic-workflow.md 第 1 期）。
        history.turn.dynamic_workflow = self.dynamic_workflow_enabled(id);
        // ListModels 的目录（第 6 期）：每轮取一次注册表视图（工具调用时不再现读）。
        history.turn.model_catalog = self
            .registry
            .as_ref()
            .map(|registry| registry.model_catalog());
        // ListWorkflowRuns 的项目键：会话工作目录（TS `context.workingDirectory`）。
        history.turn.cwd = Some(self.workspace_path.clone());
        let context = self.context.clone();
        let tools = self.tools.clone();
        let sink = Sink {
            session_id: id.into(),
            run_id,
            tx: self.events.clone(),
        };
        // model-IO 记录的调用元数据（docs/specs/rust-model-io.md）：子代理会话带 agent profile。
        let model_call = crate::contract::ModelCallScope {
            session_id: Some(id.into()),
            turn_id: Some(turn_id_for_facts.clone()),
            // TS querySourceForTask：workflow_child / subagent_child / 其余 main_turn。
            query_source: Some(
                if history.workflow_actor.is_some() {
                    "workflow_child"
                } else if history.agent_profile.is_some() {
                    "subagent"
                } else {
                    "main_turn"
                }
                .into(),
            ),
        };
        tokio::spawn(async move {
            let result = crate::contract::with_model_call(
                model_call,
                super::agent_loop::run(
                    model.as_ref(),
                    tools.as_ref(),
                    context.as_ref(),
                    &mut history,
                    &sink,
                    &cancel,
                ),
            )
            .await;
            let model_failure = result
                .as_ref()
                .err()
                .and_then(|e| e.downcast_ref::<crate::contract::ModelFailure>())
                .cloned();
            // 主轮次成功完成后调度记忆提取（TS scheduleProjectMemoryExtraction），先于 Finished 入队。
            if result.is_ok()
                && !cancel.is_cancelled()
                && let (Some(memory), Some((prefix, definitions))) =
                    (history.memory.clone(), history.memory_request.take())
            {
                let messages = history.projection(&prefix, 0, usize::MAX).0;
                let snapshot = crate::contract::MemorySnapshot {
                    memory,
                    messages,
                    definitions,
                    model: model.clone(),
                };
                let _ = sink.send(Event::MemoryExtract(Box::new(snapshot))).await;
            }
            let error = result.err().map(|e| e.to_string());
            let _ = sink
                .send(Event::Finished {
                    error,
                    model_failure,
                    cancelled: cancel.is_cancelled(),
                })
                .await;
        });
        Ok(())
    }
}

/// 会话当前运行（模型选型通道、取消与 run/turn 标识）。
pub(crate) struct Active {
    pub selection: tokio::sync::watch::Sender<crate::contract::ModelIdentity>,
    pub cancel: tokio_util::sync::CancellationToken,
    pub run_id: String,
    pub turn_id: String,
    /// 最近一次提交的模型响应 id：随后的工具调用行归属到该响应（TS toolCall `assistantResponseId`）。
    pub response_id: Option<String>,
}
