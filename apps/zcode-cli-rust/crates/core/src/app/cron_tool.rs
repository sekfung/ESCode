//! Cron 工具：守卫与参数由 domain::cron 决定，Host 反向请求经会话 owner 转发。见 docs/specs/rust-cron.md。

use crate::contract::{Event, EventSink, ToolControl, ToolOutput};
use crate::domain::cron::{self, HostError, Turn};
use crate::domain::json_order::Json;
use anyhow::{Result, anyhow, bail};
use serde_json::Value;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub(super) async fn execute(
    name: &str,
    args: &Value,
    facts: &super::context::TurnFacts,
    sink: &EventSink,
    cancel: &CancellationToken,
) -> Result<ToolOutput> {
    let turn = Turn {
        automation_turn: cron::is_automation_turn(
            facts.automation_id.as_deref(),
            &facts.disallowed,
        ),
        active_automation_id: facts.automation_id.clone(),
        bot_delivery_target: facts.bot_delivery_target.clone(),
        session_id: sink.session_id.clone(),
        mode: facts.mode.clone(),
        model_selection: facts.model_selection.clone(),
    };
    let request = cron::run(name, args, &turn, |method, params| async move {
        let (reply, answer) = oneshot::channel();
        sink.send(Event::HostRequest {
            method: method.into(),
            params,
            reply,
        })
        .await
        .map_err(|error| HostError {
            code: 0,
            message: error.to_string(),
        })?;
        match answer.await {
            Ok(Ok(raw)) => Json::parse(&raw).ok_or(HostError {
                code: 0,
                message: "Invalid Host response".into(),
            }),
            Ok(Err((code, message))) => Err(HostError { code, message }),
            Err(_) => Err(HostError {
                code: 0,
                message: "Session owner stopped before the Host replied".into(),
            }),
        }
    });
    let outcome = tokio::select! {biased;
        _ = cancel.cancelled() => bail!("Cancelled"),
        outcome = request => outcome,
    };
    if let Some(title) = outcome.freeze_title {
        sink.send(Event::FreezeTitle(title)).await?;
    }
    if outcome.limit {
        // TS withAutomationCreateLimitTurnStop：隐藏原始错误（含删除诱导），结束本轮后续工具。
        return Ok(ToolOutput {
            media: Vec::new(),
            failed: true,
            content: cron::LIMIT_MODEL_MESSAGE.into(),
            data: Value::Null,
            display: None,
            control: ToolControl {
                denied: false,
                stop_turn: true,
            },
        });
    }
    match outcome.output {
        Some((data, content)) => Ok(ToolOutput::new(content, data)),
        None => Err(anyhow!(outcome.error.unwrap_or_default())),
    }
}

/// TS：本轮禁用工具不进入模型工具面（规则名取 "(" 之前，web_search 视为 WebSearch）；
/// 子代理不注册 Cron（includeAutomation 对 subagent_child 关闭）。
pub(super) fn retain_visible(
    definitions: &mut Vec<Value>,
    facts: &super::context::TurnFacts,
    child: bool,
) {
    let disallowed: Vec<&str> = facts
        .disallowed
        .iter()
        .map(|rule| {
            let name = rule.trim();
            let name = match name.find('(') {
                Some(index) if index > 0 => &name[..index],
                _ => name,
            };
            if name == "web_search" {
                "WebSearch"
            } else {
                name
            }
        })
        .collect();
    definitions.retain(|d| {
        let name = d["function"]["name"].as_str().unwrap_or_default();
        !(disallowed.contains(&name) || (child && name.starts_with("Cron")))
    });
}
