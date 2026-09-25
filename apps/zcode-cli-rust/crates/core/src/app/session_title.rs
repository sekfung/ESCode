//! 会话标题与目标摘要标题 sidecar（docs/specs/rust-session-title.md，对齐 TS `session-title.ts`
//! 与 `goal-summary-title.ts`）。
//!
//! 普通输入在首个 run 成功后启动（TS 的 deferred 路径）；`/goal` 在 run 启动时立即启动（TS 不延迟）。
//! job 只返回候选标题，写回（`custom` 优先、首条输入回退、目标切换）始终由会话 owner 执行。
use super::{Engine, auxiliary::Auxiliary};
use crate::{
    contract::{Event, EventSink, ModelFailure, ModelIdentity},
    domain::{session_title, session_title::TitleSeed},
};
use anyhow::{Context, Result};
use serde_json::Value;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// TS `TITLE_GENERATION_TIMEOUT_MS`。
const TITLE_TIMEOUT: Duration = Duration::from_secs(60);
/// TS `AUXILIARY_MAX_OUTPUT_TOKENS`（`Math.min(5000, 模型上限)`）。
const TITLE_MAX_OUTPUT_TOKENS: usize = 5_000;

impl Engine {
    /// run 收口：普通输入只在首个 run 成功后启动一次；失败/取消与 TS 相同地丢弃 seed。
    /// 启动失败（模型未注册等）只影响标题，不改变会话结果。
    pub(super) fn finish_session_title(&mut self, id: &str) {
        let Some(s) = self.sessions.get_mut(id) else {
            return;
        };
        let succeeded = s.phase == "completedSuccess";
        let Some(seed) = s.title_seed.take().filter(|_| succeeded) else {
            return;
        };
        let _ = self.start_title(id, seed);
    }
    /// run 启动：`/goal` 的 seed 立即处理（TS 在 control-only turn 边界直接启动，不等主轮次）。
    pub(super) fn start_goal_title(&mut self, id: &str) {
        let Some(s) = self.sessions.get_mut(id) else {
            return;
        };
        if s.title_seed.as_ref().is_none_or(|seed| seed.goal_target.is_none()) {
            return;
        }
        let seed = s.title_seed.take().unwrap();
        if self.start_title(id, seed.clone()).is_err()
            && let Some(target) = seed.goal_target
        {
            // TS generateAndPersistGoalSummaryTitle 的 catch：请求无法发起时同样写兜底。
            self.goal_summary_fallback(id, &target);
        }
    }
    fn start_title(&mut self, id: &str, seed: TitleSeed) -> Result<()> {
        let (write_session, selection) = {
            let Some(s) = self.sessions.get_mut(id) else {
                return Ok(());
            };
            let interactive = s.parent_id.is_none() && s.task_type == "interactive";
            let write_session = interactive
                && !s.title_attempted
                && s.title_source != "custom"
                && session_title::should_generate(&seed);
            let goal = interactive && session_title::should_generate_goal_summary(&seed);
            if write_session {
                s.title_attempted = true;
            }
            if !write_session && !goal {
                // TS maybeStartGoalSummaryTitleGeneration 不满足条件时直接写兜底摘要。
                if let Some(target) = &seed.goal_target {
                    self.goal_summary_fallback(id, target);
                }
                return Ok(());
            }
            let selection = ModelIdentity {
                provider_id: s.provider.clone(),
                model_id: s.model.clone(),
                reasoning_level: s.reasoning_level.clone(),
            };
            (write_session, selection)
        };
        let model = match &self.registry {
            Some(registry) => registry.resolve(&selection)?,
            None => self.model.clone().context("Model required")?,
        };
        // TS auxiliaryModelOptions：最低推理档位绑定 + min(5000, 模型上限)。
        let model = model.auxiliary().unwrap_or(model);
        let model = model
            .with_max_output_tokens(TITLE_MAX_OUTPUT_TOKENS)?
            .unwrap_or(model);
        let cancel = CancellationToken::new();
        let job = format!("session-title:{}", self.clock.id());
        self.auxiliary.insert(
            job.clone(),
            Auxiliary {
                request: None,
                cancel: cancel.clone(),
                operation: None,
                session: Some(id.to_owned()),
            },
        );
        self.title_jobs.insert(id.to_owned(), job.clone());
        let sink = EventSink {
            session_id: job.clone(),
            run_id: job,
            tx: self.events.clone(),
        };
        let session = id.to_owned();
        let messages = session_title::request_messages(&seed);
        tokio::spawn(async move {
            let result = tokio::select! {
                _ = cancel.cancelled() => Err(ModelFailure::cancelled()),
                outcome = tokio::time::timeout(TITLE_TIMEOUT, model.complete(messages, &[], &sink, &cancel)) => match outcome {
                    Ok(Ok(output)) => Ok(output),
                    Ok(Err(failure)) => Err(failure),
                    Err(_) => Err(ModelFailure::new("timeout", false)),
                },
            };
            // 返回工具调用或清洗后为空都视为跳过：不改会话标题、不重试（TS 同规则）。
            let title = match result {
                Ok(output) if output.calls.is_empty() => output.message["content"]
                    .as_str()
                    .and_then(session_title::clean_generated_title),
                _ => None,
            };
            if title.is_some() || seed.goal_target.is_some() {
                let _ = sink
                    .send(Event::SessionTitle {
                        session,
                        entity: seed.entity,
                        title,
                        write_session,
                        goal_target: seed.goal_target,
                    })
                    .await;
            }
            let _ = sink
                .send(Event::AuxiliaryDone {
                    result: Ok(Value::Null),
                })
                .await;
        });
        Ok(())
    }
    /// 写回 sidecar 结果：会话标题 `custom` 优先、首条输入被回退后放弃；目标摘要按目标 id 校验。
    pub(super) async fn apply_session_title(
        &mut self,
        id: &str,
        entity: &str,
        title: Option<&str>,
        write_session: bool,
        goal_target: Option<&str>,
    ) -> Result<()> {
        let Some(s) = self.sessions.get_mut(id) else {
            return Ok(());
        };
        let mut changed = false;
        if let Some(title) = title.filter(|_| write_session)
            && s.parent_id.is_none()
            && s.task_type == "interactive"
            && s.title_source != "custom"
            && s.title != title
            // TS hasEditedFirstVisibleUserQuery：首条输入被编辑/回退（history cut 移出 inputs）后不再写回。
            && s.history.inputs.iter().any(|input| input.entity == entity)
        {
            s.title = title.to_owned();
            s.title_source = "generated".into();
            changed = true;
        }
        if let Some(target) = goal_target {
            changed |= match title {
                // TS persistGeneratedGoalSummaryTitle：目标仍是同一个且标题有变化时写入。
                Some(title) => s
                    .goal
                    .as_mut()
                    .filter(|g| g.target_id == target && g.summary_title.as_deref() != Some(title))
                    .map(|g| g.summary_title = Some(title.to_owned()))
                    .is_some(),
                None => fallback(s, target),
            };
        }
        if changed {
            s.updated_at = self.clock.now();
            s.revision += 1;
            self.publish(id, vec![])?;
            self.persist(id, None).await?;
        }
        Ok(())
    }
    /// 不发请求时的兜底摘要：写入内存态并随下一次提交持久化（run 启动即会提交）。
    fn goal_summary_fallback(&mut self, id: &str, target: &str) {
        if let Some(s) = self.sessions.get_mut(id)
            && fallback(s, target)
        {
            s.revision += 1;
        }
    }
    /// 会话关闭：取消未完成的标题请求。
    pub(super) fn shutdown_session_title(&mut self, id: &str) {
        if let Some(job) = self.title_jobs.remove(id)
            && let Some(entry) = self.auxiliary.get(&job)
        {
            entry.cancel.cancel();
        }
    }
}

/// TS persistFallbackGoalSummaryTitle：目标仍是同一个且尚无摘要标题时写入兜底。
fn fallback(s: &mut crate::domain::session::Session, target: &str) -> bool {
    let Some(goal) = s.goal.as_mut().filter(|g| g.target_id == target) else {
        return false;
    };
    if goal.summary_title.as_deref().is_some_and(|t| !t.trim().is_empty()) {
        return false;
    }
    match session_title::fallback_goal_summary_title(&goal.objective) {
        Some(title) => {
            goal.summary_title = Some(title);
            true
        }
        None => false,
    }
}
