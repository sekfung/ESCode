use super::Engine;
use crate::contract::{Event, EventSink as Sink, ModelPort};
use anyhow::{Context, Result};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

impl Engine {
    pub(super) fn start_run(&mut self, id: &str, turn_id: String) -> Result<()> {
        let turn_id_for_facts = turn_id.clone();
        let identity = self.session_selection(id)?;
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
        let cancel = CancellationToken::new();
        self.active.insert(
            id.into(),
            Active {
                selection,
                cancel: cancel.clone(),
                run_id: run_id.clone(),
                turn_id,
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
        history.skills = session.skills.clone();
        history.goal = session.goal.clone();
        history.agent_profile = session.agent_profile.clone();
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
            model_selection: Some(if session.reasoning_level.is_empty() {
                serde_json::json!({"providerId": session.provider, "modelId": session.model})
            } else {
                serde_json::json!({"providerId": session.provider, "modelId": session.model, "options": {"reasoningLevel": session.reasoning_level}})
            }),
        };
        let context = self.context.clone();
        let tools = self.tools.clone();
        let sink = Sink {
            session_id: id.into(),
            run_id,
            tx: self.events.clone(),
        };
        tokio::spawn(async move {
            let result = super::agent_loop::run(
                model.as_ref(),
                tools.as_ref(),
                context.as_ref(),
                &mut history,
                &sink,
                &cancel,
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
}
