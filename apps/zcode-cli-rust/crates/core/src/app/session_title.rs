//! 会话标题 sidecar（docs/specs/rust-session-title.md，对齐 TS `maybeStartDeferredSessionTitleGeneration`）。
//!
//! 首个 run 收口后由会话 owner 启动一次辅助调用；job 只返回候选标题，写回（含 `custom` 与
//! 首条输入回退的校验）始终由 owner 执行，保证标题只有一个写入者。
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
    /// run 收口：只在首个 run 成功后取走 seed 启动一次 sidecar；失败/取消与 TS 相同地丢弃 seed。
    /// 启动失败（模型未注册等）只影响标题，与 TS 一样不改变会话结果。
    pub(super) fn finish_session_title(&mut self, id: &str) {
        let Some(s) = self.sessions.get_mut(id) else {
            return;
        };
        let succeeded = s.phase == "completedSuccess";
        let Some(seed) = s.title_seed.take().filter(|_| succeeded) else {
            return;
        };
        let _ = self.start_session_title(id, seed);
    }
    /// 首个 run 成功后启动标题 sidecar；已尝试/子代理/非交互会话不触发。
    pub(super) fn start_session_title(&mut self, id: &str, seed: TitleSeed) -> Result<()> {
        if !session_title::should_generate(&seed) {
            return Ok(());
        }
        let selection = {
            let Some(s) = self.sessions.get_mut(id) else {
                return Ok(());
            };
            if s.title_attempted
                || s.parent_id.is_some()
                || s.task_type != "interactive"
                || s.title_source == "custom"
            {
                return Ok(());
            }
            s.title_attempted = true;
            ModelIdentity {
                provider_id: s.provider.clone(),
                model_id: s.model.clone(),
                reasoning_level: s.reasoning_level.clone(),
            }
        };
        let model = match &self.registry {
            Some(registry) => registry.resolve(&selection)?,
            None => self.model.clone().context("Model required")?,
        };
        // TS auxiliaryModelOptions：最低推理档位绑定 + min(5000, 模型上限)。
        let model = model.auxiliary().unwrap_or(model);
        let model = model.with_max_output_tokens(TITLE_MAX_OUTPUT_TOKENS)?.unwrap_or(model);
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
        let entity = seed.entity.clone();
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
            // 返回工具调用或清洗后为空都视为跳过：不改标题、不重试（TS 同规则）。
            let title = match result {
                Ok(output) if output.calls.is_empty() => output.message["content"]
                    .as_str()
                    .and_then(session_title::clean_generated_title),
                _ => None,
            };
            if let Some(title) = title {
                let _ = sink
                    .send(Event::SessionTitle {
                        session,
                        entity,
                        title,
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
    /// 写回候选标题：`custom` 优先，首条输入被回退后放弃。
    pub(super) async fn apply_session_title(
        &mut self,
        id: &str,
        entity: &str,
        title: &str,
    ) -> Result<()> {
        let Some(s) = self.sessions.get_mut(id) else {
            return Ok(());
        };
        if s.parent_id.is_some()
            || s.task_type != "interactive"
            || s.title_source == "custom"
            || title.is_empty()
            || s.title == title
        {
            return Ok(());
        }
        // TS hasEditedFirstVisibleUserQuery：首条输入被编辑/回退（history cut 移出 inputs）后不再写回。
        if !s.history.inputs.iter().any(|input| input.entity == entity) {
            return Ok(());
        }
        s.title = title.to_owned();
        s.title_source = "generated".into();
        s.updated_at = self.clock.now();
        s.revision += 1;
        self.publish(id, vec![])?;
        self.persist(id, None).await?;
        Ok(())
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
