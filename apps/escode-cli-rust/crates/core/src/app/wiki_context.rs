//! LLM Wiki 的 runtime 接入（apps/escode-cli-rust/docs/llm-wiki/01-spec.md §9–§10）。
//! - 工具面：run 开始时向端口询问一次可见性，决定是否追加四个 Wiki 工具（会话内不变）。
//! - 每轮注入：主会话（非子代理/工作流 actor）有真实用户输入时检索；结果作为隐藏上下文经 owner 提交并落库，
//!   插在本轮输入之前（与 hooks 上下文同一路径），提交回执后才继续请求模型。
//! - 去重：本会话已注入的 `id@version` 直接从上下文中的 wiki 消息解析；压缩后这些消息离开上下文，自然允许重新注入。
//! - 失败 fail-open：检索出错或超时不影响本轮执行（诊断日志在适配器侧）。
use super::context::RunContext;
use crate::contract::{Event, EventSink, WikiSurface, WikiTurnInput};
use crate::domain::wiki::reminder::{SOURCE, injected_versions};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

fn main_session(history: &RunContext) -> bool {
    history.agent_profile.is_none() && history.workflow_actor.is_none()
}

/// 询问端口并按需追加工具定义；子代理按其 profile 的工具白名单过滤。
pub(super) async fn prepare(history: &RunContext, definitions: &mut Vec<Value>) -> WikiSurface {
    let Some(port) = history.turn.wiki.clone() else {
        return WikiSurface::default();
    };
    let surface = port.surface().await;
    if surface.tools {
        for definition in crate::domain::wiki::tools::definitions() {
            let name = definition["function"]["name"].as_str().unwrap_or_default().to_owned();
            let allowed = history.agent_profile.as_ref().is_none_or(|p| p.allows(&name));
            let present = definitions.iter().any(|d| d["function"]["name"] == name.as_str());
            if allowed && !present && !history.turn.disallowed.contains(&name) {
                definitions.push(definition);
            }
        }
    }
    surface
}

pub(super) async fn before_turn(
    history: &mut RunContext,
    surface: WikiSurface,
    sink: &EventSink,
    cancel: &CancellationToken,
) -> Result<()> {
    let (Some(port), Some(prompt)) = (history.turn.wiki.clone(), history.turn.prompt.clone()) else {
        return Ok(());
    };
    if !surface.auto_inject || !main_session(history) || prompt.trim().is_empty() {
        return Ok(());
    }
    let contents: Vec<&str> = history
        .messages
        .iter()
        .filter(|m| m["_escode_source"] == SOURCE)
        .filter_map(|m| m["content"].as_str())
        .collect();
    let input = WikiTurnInput {
        session_id: sink.session_id.clone(),
        prompt,
        already_injected: injected_versions(&contents),
    };
    let retrieval = match port.retrieve_for_turn(input, cancel).await {
        Ok(retrieval) => retrieval,
        Err(_) if cancel.is_cancelled() => bail!("Cancelled"),
        // fail-open：知识检索失败不阻断本轮（适配器已记录诊断）。
        Err(_) => return Ok(()),
    };
    let Some(body) = retrieval.body else {
        return Ok(());
    };
    let message = json!({
        "role": "user",
        "content": crate::domain::plan_mode::wrap(&body),
        "_escode_source": SOURCE,
    });
    let (committed, receipt) = oneshot::channel();
    sink.send(Event::WikiContext {
        message: message.clone(),
        committed,
    })
    .await?;
    tokio::select! {biased;
        _ = cancel.cancelled() => bail!("Cancelled"),
        result = receipt => result.context("Session owner stopped before wiki context commit")?,
    }
    let at = super::turn_hooks::input_start(&history.messages);
    history.insert(at, message);
    Ok(())
}

/// Wiki 工具分派：端口缺席时明确失败（不是「没有知识」）。
pub(super) async fn execute(
    history_wiki: Option<&dyn crate::contract::WikiPort>,
    name: &str,
    input: &Value,
    session: &str,
) -> Result<crate::contract::ToolOutput> {
    let Some(port) = history_wiki else {
        return Ok(crate::contract::ToolOutput {
            failed: true,
            ..crate::contract::ToolOutput::text(
                "wiki_unavailable: this runtime has no LLM Wiki configured; this is a capability gap, not an empty knowledge base.".to_owned(),
            )
        });
    };
    let result = port.tool(name, input, session).await?;
    let mut output = crate::contract::ToolOutput::new(result.content, result.structured);
    output.failed = result.failed;
    Ok(output)
}
