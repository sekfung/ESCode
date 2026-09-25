//! 单个工具调用的派发：权限结论、按工具名路由到会话侧实现或 ToolPort。
use crate::contract::{Event, EventSink, ModelPort, ToolPort};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub(super) struct ExecutionContext<'a> {
    pub skills: &'a crate::domain::skills::SkillCatalog,
    pub profile: Option<&'a crate::domain::subagent::Profile>,
    pub profiles: &'a [crate::domain::subagent::Profile],
    pub selection: Option<crate::contract::ModelIdentity>,
    pub model: &'a dyn ModelPort,
    pub turn: &'a super::context::TurnFacts,
    /// 主会话启用记忆时的记忆根（权限放行记忆 Markdown 写入）。
    pub memory_root: Option<&'a str>,
}
pub(super) async fn execute(
    tools: &dyn ToolPort,
    context: ExecutionContext<'_>,
    call: Value,
    sink: &EventSink,
    cancel: &CancellationToken,
) -> Result<(String, String, crate::contract::ToolOutput, bool, bool)> {
    let ExecutionContext {
        skills,
        profile,
        profiles,
        selection,
        model,
        turn,
        memory_root,
    } = context;
    if cancel.is_cancelled() {
        bail!("Cancelled");
    }
    sink.send(Event::ToolStart { call: call.clone() }).await?;
    let name = call["function"]["name"]
        .as_str()
        .context("Tool name missing")?;
    // 判定统一由会话 owner 完成（模式、规则与确认交互都在那里）；这里只消费结论。
    let outcome = {
        let (reply, receipt) = oneshot::channel();
        sink.send(Event::Permission {
            call: call.clone(),
            memory_root: memory_root.map(str::to_owned),
            reply,
        })
        .await?;
        tokio::select! {biased;
            _=cancel.cancelled()=>bail!("Cancelled"),
            result=receipt=>result.unwrap_or_else(|_| crate::contract::PermissionOutcome::deny(
                crate::domain::permission_options::denied_content(None))),
        }
    };
    let result = if profile.is_some_and(|p| !p.allows(name)) {
        Err(anyhow::anyhow!(
            "Tool is not allowed by this subagent profile"
        ))
    } else if !outcome.allowed {
        // 拒绝文案与 TS 逐字一致（不套 "Tool failed:" 前缀），模型据此停止并等待用户指示。
        Ok(crate::contract::ToolOutput {
            media: Vec::new(),
            failed: true,
            content: outcome.denial.unwrap_or_else(|| "Permission denied".into()),
            data: Value::Null,
            display: None,
            control: crate::contract::ToolControl {
                denied: true,
                stop_turn: false,
            },
        })
    } else {
        match serde_json::from_str::<Value>(call["function"]["arguments"].as_str().unwrap_or("")) {
            Ok(args)
                if matches!(name, "Agent" | "Task" | "SendMessage")
                    || matches!(name, "TaskOutput" | "TaskStop")
                        && args["task_id"]
                            .as_str()
                            .is_some_and(|id| id.starts_with("agent_")) =>
            {
                super::subagent_tools::execute(
                    (tools, profiles, skills),
                    name,
                    &args,
                    call["id"].as_str().unwrap(),
                    selection,
                    sink,
                    cancel,
                )
                .await
            }
            Ok(args) if matches!(name, "EnterPlanMode" | "ExitPlanMode") => {
                super::plan_tool::execute(name, call["id"].as_str().unwrap(), args, sink, cancel)
                    .await
            }
            Ok(args) if name == "AskUserQuestion" => {
                super::question_tool::execute(call["id"].as_str().unwrap(), args, sink, cancel)
                    .await
            }
            Ok(args) if matches!(name, "TodoRead" | "TodoWrite") => {
                super::todos::execute(name, call["id"].as_str().unwrap(), args, sink, cancel).await
            }
            Ok(args) if name.starts_with("Cron") => {
                super::cron_tool::execute(name, &args, turn, sink, cancel).await
            }
            Ok(args) if name == "ReadSessionContext" => {
                super::session_context_tool::execute(model, &args, sink, cancel).await
            }
            Ok(args) if name == "WebSearch" => {
                super::web_search_tool::execute(model, &args, sink, cancel).await
            }
            Ok(args) if name == "WebFetch" => {
                super::web_fetch_tool::execute(tools, model, &args, sink, cancel).await
            }
            Ok(args) if name == "Skill" => {
                super::skills::execute(tools, skills, &args, cancel).await
            }
            Ok(args) if name.starts_with("mcp__") => {
                let call_id = call["id"].as_str().unwrap();
                tools.execute_mcp(name, &args, call_id, sink, cancel).await
            }
            Ok(args) => tools.execute_scoped(name, &args, sink, cancel).await,
            Err(_) => Err(anyhow::anyhow!("Invalid tool JSON arguments")),
        }
    };
    if let Err(error) = &result
        && error.is::<crate::contract::ProcessCleanupFailure>()
    {
        // 进程未确认回收时不能包装成普通工具失败再发请求；由 owner 终止本 runtime。
        sink.send(Event::ToolCleanupFailed(format!("{error:#}")))
            .await?;
        return Err(result.err().unwrap());
    }
    let failed = result.as_ref().map_or(true, |output| output.failed);
    let denied = result.as_ref().is_ok_and(|output| output.control.denied);
    let content = result
        .unwrap_or_else(|error| crate::contract::ToolOutput::text(format!("Tool failed: {error}")));
    Ok((
        call["id"].as_str().context("Tool id missing")?.into(),
        name.to_owned(),
        content,
        failed,
        denied,
    ))
}
