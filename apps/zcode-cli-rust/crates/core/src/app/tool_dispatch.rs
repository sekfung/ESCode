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
    /// 本轮发给模型的工具定义；执行前按其 parameters 校验入参。
    pub definitions: &'a [Value],
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
        definitions,
    } = context;
    if cancel.is_cancelled() {
        bail!("Cancelled");
    }
    let display = call["function"]["name"]
        .as_str()
        .filter(|n| n.starts_with("mcp__"))
        .and_then(|n| tools.mcp_tool(&sink.session_id, n)?.display);
    sink.send(Event::ToolStart { call: call.clone(), display }).await?;
    let tool_name = call["function"]["name"]
        .as_str()
        .context("Tool name missing")?
        .to_owned();
    let name = tool_name.as_str();
    let mut call = call;
    // TS validateInitialModelToolInput：入参先按发给模型的定义 parameters 校验，失败时不请求权限、不调用工具，
    // 把问题回传模型（docs/specs/rust-tool-input-validation.md；Rust 之前 MCP 原样透传、内置工具用各自文案）。
    match checked_input(definitions, name, &call) {
        Err(content) => {
            let output = crate::contract::ToolOutput {
                failed: true,
                ..crate::contract::ToolOutput::text(content)
            };
            return Ok((call["id"].as_str().context("Tool id missing")?.into(), tool_name, output, true, false));
        }
        Ok(Some(arguments)) => call["function"]["arguments"] = arguments.into(),
        Ok(None) => {}
    }
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
            // 闲时受限轮（docs/specs/rust-offpeak.md 第二期）：SendMessage 会绕开本轮执行模型续跑子代理；
            // Bash 后台命令完成后的通知轮会落到用户套餐，拒绝显式后台并关闭超时自动转后台。
            Ok(_) if turn.off_peak_restricted && name == "SendMessage" => Err(anyhow::anyhow!(
                crate::domain::off_peak::off_peak_turn_denial(
                    name,
                    Some(crate::domain::off_peak::SEND_MESSAGE_HINT)
                )
            )),
            Ok(args) if turn.off_peak_restricted && name == "Bash" && args["run_in_background"] == true => {
                Err(anyhow::anyhow!(crate::domain::off_peak::BASH_BACKGROUND_DENIED))
            }
            Ok(mut args) if turn.off_peak_restricted && name == "Bash" => {
                args[crate::domain::off_peak::FOREGROUND_ONLY_ARG] = true.into();
                tools.execute_scoped(name, &args, sink, cancel).await
            }
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
            Ok(args) if name.starts_with("OffPeak") => {
                super::off_peak::execute(name, &args, turn, sink, cancel).await
            }
            Ok(args) if name.starts_with("Cron") => {
                super::cron_tool::execute(name, &args, turn, sink, cancel).await
            }
            Ok(args) if name == "ReadSessionContext" => {
                super::session_context_tool::execute(model, &args, sink, cancel).await
            }
            Ok(args) if name == "ListModels" => {
                super::model_catalog_tool::execute(
                    turn.model_catalog.as_deref(),
                    selection.as_ref(),
                )
            }
            Ok(args) if name == "ListWorkflowRuns" => {
                super::workflow_run_tool::execute(&args, turn.cwd.as_deref(), sink, cancel).await
            }
            Ok(args) if name == "WebSearch" => {
                super::web_search_tool::execute(model, &args, sink, cancel).await
            }
            Ok(args) if name == "WebFetch" => {
                super::web_fetch_tool::execute(
                    tools,
                    model,
                    &args,
                    call["id"].as_str().unwrap_or_default(),
                    sink,
                    cancel,
                )
                .await
            }
            Ok(args) if name == "Skill" => {
                super::skills::execute(tools, skills, &args, cancel).await
            }
            Ok(args) if name.starts_with("mcp__") => {
                let call_id = call["id"].as_str().unwrap();
                tools.execute_mcp(name, &args, call_id, &turn.mcp_meta, sink, cancel).await
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
    // TS serializeOutput：模型可见内容为空白时换成通用占位，避免模型把静默成功误读成缺失结果
    // （docs/specs/rust-bash-model-content.md；Rust 之前直接发空串）。
    let mut result = result;
    if let Ok(output) = &mut result
        && !output.control.denied
        && output.media.is_empty()
        && output.content.trim().is_empty()
    {
        output.content = format!("({name} completed with no output)");
    }
    let failed = result.as_ref().map_or(true, |output| output.failed);
    let denied = result.as_ref().is_ok_and(|output| output.control.denied);
    let content = result
        // TS createErrorResult：处理器失败套 <tool_use_error>，其余错误为消息原文（Rust 之前统一加 "Tool failed: "
        // 前缀，docs/specs/rust-file-tool-results.md）。
        .unwrap_or_else(|error| {
            crate::contract::ToolOutput::text(match error.downcast_ref::<crate::contract::ToolHandlerFailure>() {
                Some(failure) => format!("<tool_use_error>{failure}</tool_use_error>"),
                None => crate::domain::tool_failure::plain_error_text(&error.to_string()),
            })
        });
    Ok((
        call["id"].as_str().context("Tool id missing")?.into(),
        name.to_owned(),
        content,
        failed,
        denied,
    ))
}

/// `Err(文案)`：校验失败；`Ok(Some(参数原文))`：去掉未知键后的参数；`Ok(None)`：原样执行。
/// 定义中没有该工具时不校验。
fn checked_input(definitions: &[Value], name: &str, call: &Value) -> Result<Option<String>, String> {
    use crate::domain::{json_order::Json, schema_order, tool_input_validation as validation};
    let Some(schema) = definitions
        .iter()
        .find(|d| d["function"]["name"] == name)
        .and_then(|d| schema_order::ordered(&d["function"]["parameters"]))
    else {
        return Ok(None);
    };
    // 参数按原文解析以保留键顺序（多余参数按模型给出的顺序列出）；非法 JSON 交给原有路径报错。
    let Some(args) = call["function"]["arguments"].as_str().and_then(Json::parse) else {
        return Ok(None);
    };
    use crate::domain::tool_input_strip::{fill_defaults, strip};
    // TS：内置工具先经 runtime schema（丢弃未知键、填默认值），通过后再按 JSON Schema 校验；
    // runtime 失败时按原始参数报告（有默认值的缺失属性不报）。MCP 工具按原始参数校验。
    let builtin = !name.starts_with("mcp__");
    let normalize = |value: &Json| {
        let mut value = value.clone();
        if builtin {
            fill_defaults(&mut value, &schema);
        }
        value
    };
    // TS runtime schema 的 preprocess/transform 先于丢弃未知键与 JSON 校验；执行也用转换后的参数。
    let coerced = if builtin { crate::domain::tool_input_strip::coerce(name, &args) } else { args.clone() };
    let stripped = strip(name, &coerced, &schema);
    if !validation::validate(&normalize(&stripped), &schema).is_empty() {
        let issues = validation::validate(&normalize(&args), &schema);
        if !issues.is_empty() {
            return Err(validation::model_content(name, &issues));
        }
    }
    Ok((stripped != args).then(|| stripped.compact()))
}
