use super::event_projection_dispatch::DispatchedRunEvent;
use super::{Engine, Event, RunEvent};
use anyhow::{Result, bail};
use serde_json::json;
impl Engine {
    pub(super) async fn apply_event(&mut self, event: RunEvent) -> Result<()> {
        let Some(dispatched) = self.dispatch_run_event(event).await? else {
            return Ok(());
        };
        let DispatchedRunEvent {
            mut event,
            id,
            turn,
        } = dispatched;
        // 工具结果媒体先落附件存储，会话消息只保留引用（base64 不进入会话库）。
        let stored = match &mut event.event {
            Event::ToolDone { media, .. } if !media.is_empty() => {
                Some(self.store_tool_media(std::mem::take(media)).await?)
            }
            _ => None,
        };
        self.forward_actor_event(&id, &event.event).await;
        self.record_usage_facts(&id, &turn, &event.event);
        self.telemetry_event(&id, &turn, &event.event);
        if matches!(event.event, Event::ModelStatus(_)) {
            return Ok(());
        }
        let now = self.clock.now();
        let s = self.sessions.get_mut(&id).unwrap();
        let mut deltas = vec![];
        let mut finished = false;
        let text_only = matches!(event.event, Event::Text { .. });
        let mut receipt = None;
        match event.event {
            Event::FilePrepared { .. }
            | Event::Todos { .. }
            | Event::Subagent { .. }
            | Event::GoalStep { .. }
            | Event::GoalVerdict { .. }
            | Event::SkillsInitialized { .. }
            | Event::TodoReminder { .. }
            | Event::PersistedNotice { .. }
            | Event::HookContext { .. }
            | Event::WikiContext { .. }
            | Event::ShellNotice { .. }
            | Event::StreamRecovery { .. }
            | Event::Question { .. }
            | Event::PlanEnter { .. }
            | Event::PlanExit { .. }
            | Event::ToolCleanupFailed(_)
            | Event::StepBoundary { .. }
            | Event::Background { .. }
            | Event::PromptInitialized { .. }
            | Event::AuxiliaryDone { .. }
            | Event::AuxiliaryReply { .. }
            | Event::AuxiliaryNotify { .. }
            | Event::RequestAuth { .. }
            | Event::ShellPreference { .. }
            | Event::MemoryPreference { .. }
            | Event::MemoryResolved(_)
            | Event::MemoryExtract(_)
            | Event::SessionContext { .. }
            | Event::WorkflowRunList { .. }
            | Event::HostRequest { .. }
            | Event::WorkflowSettled { .. }
            | Event::ActorRequest { .. }
            | Event::ContextUsage(_)
            | Event::CompactStarted { .. }
            | Event::CompactDone { .. }
            | Event::SessionTitle { .. }
            | Event::ModelStatus(_)
            | Event::WorkflowLaunchTurn { .. }
            | Event::WorkflowSettingsTurn { .. } => unreachable!(),
            Event::Retry(state) => {
                s.api_retry = state;
            }
            // TS setCustomSessionTitle：CronCreate 成功后标题固定为 automation 标题，阻止后续生成标题覆盖。
            Event::FreezeTitle(title) => {
                s.title = title;
                s.title_source = "custom".into();
                s.revision += 1;
            }
            Event::Text {
                response_id,
                text,
                reasoning,
            } => {
                let kind = if reasoning {
                    "reasoning"
                } else {
                    "assistantText"
                };
                if let Some(row) = s.text_row_to_continue(&response_id, kind) {
                    let serde_json::Value::String(current) = &mut row["text"] else {
                        bail!("Invalid text projection");
                    };
                    if current.len() + text.len() > crate::domain::MAX_TEXT_BYTES {
                        bail!("Projected text exceeds limit");
                    }
                    current.push_str(&text);
                    deltas.push(
                        json!({"op":"row.delta","rowId":row["rowId"],"path":"text","append":text}),
                    );
                } else {
                    let mut row = s.row(kind, &turn, &self.clock.id(), now);
                    row["assistantResponseId"] = response_id.into();
                    row["text"] = text.into();
                    row["state"] = "streaming".into();
                    s.rows.push(row.clone());
                    deltas.push(json!({"op":"row.appended","row":row}));
                }
            }
            Event::ModelDone {
                response_id,
                stable,
                message,
                usage,
                committed,
            } => {
                receipt = Some(committed);
                if let Some(active) = self.active.get_mut(&id) {
                    active.response_id = Some(response_id);
                }
                if let Some(message) = message {
                    s.append_message(message);
                }
                for row in &mut s.rows {
                    if row["turnId"] == turn && row["state"] == "streaming" {
                        row["state"] = "complete".into();
                        deltas.push(json!({"op":"row.upserted","row":row}));
                    }
                }
                if stable {
                    s.record_response(&turn);
                }
                super::goal_events::account_usage(s, &usage, now);
                // v3.15.1 TS onModelComplete：主会话的单 Turn 输出 token 在权威事件投影中累计到 turnHeader。
                if s.agent_profile.is_none()
                    && let Some(row) = s
                        .rows
                        .iter_mut()
                        .find(|r| r["kind"] == "turnHeader" && r["turnId"] == turn)
                {
                    let output = usage["completion_tokens"].as_u64().unwrap_or(0);
                    row["outputTokens"] =
                        (row["outputTokens"].as_u64().unwrap_or(0) + output).into();
                    deltas.push(json!({"op":"row.upserted","row":row}));
                }
                if s.goal.as_ref().is_some_and(|g| g.active() && g.exhausted()) {
                    s.goal.as_mut().unwrap().pause(now);
                    // 预算耗尽的 assistant 事实先提交；取消后绝不能再执行它声明的工具。
                    self.active[&id].cancel.cancel();
                }
            }
            Event::ToolStart { call, display } => {
                let response = self.active[&id].response_id.clone();
                let mut row = s.tool_call_row(&turn, &call, response, now);
                if let Some(display) = display {
                    row["display"] = display;
                }
                s.rows.push(row.clone());
                deltas.push(json!({"op":"row.appended","row":row}));
            }
            Event::Permission {
                call,
                memory_root,
                approval_proceed,
                hook,
                reply,
            } => {
                self.ask_permission(
                    &id,
                    &event.run_id,
                    &turn,
                    &call,
                    memory_root.as_deref(),
                    (approval_proceed, hook),
                    reply,
                )
                .await?;
                return Ok(());
            }
            Event::ToolDone {
                id: call_id,
                tool,
                result,
                display,
                failed,
                denied,
                committed,
                ..
            } => {
                receipt = Some(committed);
                let mut message = json!({"role":"tool","tool_call_id":call_id,"content":result,"_escode_tool_failed":failed});
                if let Some(parts) = stored {
                    message["content"] = parts.into();
                    message["_escode_tool_name"] = tool.into();
                }
                s.append_message(message);
                let done = crate::domain::ToolResult {
                    result,
                    display,
                    failed,
                    denied,
                };
                if let Some(row) = s.finish_tool_row(&turn, &call_id, done, now) {
                    deltas.push(json!({"op":"row.upserted","row":row}));
                }
            }
            Event::BrowserTurnScreenshot { display, committed } => {
                receipt = Some(committed);
                let row = s.turn_screenshot_row(
                    &turn,
                    self.active[&id].response_id.clone(),
                    display,
                    &self.clock.id(),
                    now,
                );
                deltas.push(json!({"op":"row.appended","row":row}));
            }
            Event::Finished {
                error,
                model_failure,
                cancelled,
            } => {
                // cancel 可在 loop 正常返回与 Finished 入队之间到达，以 owner 的取消事实为准。
                let cancelled = cancelled || self.active[&id].cancel.is_cancelled();
                if let Some(goal) = &mut s.goal {
                    if cancelled || error.is_some() {
                        goal.pause(now);
                    } else {
                        goal.settle(now);
                    }
                }
                super::busy_input::fallback_guides(
                    s,
                    if cancelled || error.is_some() {
                        "guide.turnInterrupted"
                    } else {
                        "guide.noToolBoundary"
                    },
                );
                finished = true;
                s.api_retry = None;
                s.run_id = None;
                s.pending.clear();
                s.revision += 1;
                let outcome = if cancelled {
                    "interrupted"
                } else if error.is_some() {
                    "failed"
                } else {
                    "success"
                };
                s.phase = match outcome {
                    "interrupted" => "completedInterrupted",
                    "failed" => "error",
                    _ => "completedSuccess",
                }
                .into();
                if cancelled || error.is_some() {
                    if s.queued_now.is_none() {
                        s.auto_drain = false;
                    }
                    s.close_unfinished_tools();
                }
                if !cancelled && let Some(message) = error {
                    s.last_error = Some(
                        json!({"code":"fault.runtime.execution","message":message,"recoverable":true,"at":now,"source":"runtime"}),
                    );
                    if let Some(failure) = model_failure {
                        s.last_error = Some(json!({"code":failure.code,"message":failure.message,
                            "recoverable":failure.retryable,"at":now,"source":"provider",
                            "attribution":{"source":"provider","reason":failure.reason,"providerId":s.provider,"modelId":s.model,"retryable":failure.retryable}}));
                        if let Some(status) = failure.status_code {
                            s.last_error.as_mut().unwrap()["attribution"]["statusCode"] =
                                status.into();
                        }
                        s.last_model_failure = Some(failure);
                    }
                }
                s.finish_rows(outcome, now);
                deltas.extend(
                    s.rows
                        .iter()
                        .filter(|r| r["turnId"] == turn)
                        .map(|r| json!({"op":"row.upserted","row":r})),
                );
                self.waiters.release(&id);
                self.active.remove(&id);
            }
        }
        s.updated_at = now;
        let checkpoint = !text_only || now.saturating_sub(s.checkpoint_at) >= 250;
        if checkpoint {
            s.checkpoint_at = now;
        }
        if finished {
            deltas.extend(
                super::file_changes::hydrate(
                    self.sessions.get_mut(&id).unwrap(),
                    self.tools.as_ref(),
                    Some(&turn),
                )
                .await?,
            );
        }
        self.publish(&id, deltas)?;
        if checkpoint {
            self.persist(&id, None).await?;
        }
        if let Some(receipt) = receipt {
            let _ = receipt.send(());
        }
        if finished {
            self.promote(&id).await?;
            self.deliver_children(&id).await?;
            self.deliver_workflow_notices(&id).await?;
            self.finish_actor_turn(&id);
            self.finish_child(&id).await?;
            self.finish_session_title(&id);
        }
        Ok(())
    }
}
