//! 单个工具调用的派发：权限结论、按工具名路由到会话侧实现或 ToolPort。
use super::tool_input_checks::{checked_input, prepare, resolve_registered};
use crate::contract::{Event, EventSink, ModelPort, ToolPort};
use anyhow::{Context, Result, bail};
use escode_cli_domain::tool_search_catalog::{
    DEFERRED_TOOL_CALL_NAME, TOOL_SEARCH_NAME, resolve_deferred_call, search_tools,
    tool_reference_blocks,
};
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
    /// 本会话注册的工具（可见性过滤之前）：查不到的调用回 `Tool not found`。
    pub registered: &'a [Value],
    /// 会话历史里是否成功加载过 `dynamic-workflows` 技能（TS 技能门 `hasLoadedSkill`）。
    pub workflow_skill_loaded: bool,
    /// 本轮 ToolSearch 决策；None 表示该能力未启用。
    pub tool_search: Option<&'a super::agent_loop_surface::ToolSearchContext>,
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
        registered: context_registered,
        workflow_skill_loaded,
        tool_search,
    } = context;
    // 工作流宿主的 actor 模型基线（TS ModelSelection 形状）。
    let selection_json = selection.as_ref().map_or(Value::Null, |s| {
        serde_json::json!({"providerId": s.provider_id, "modelId": s.model_id, "options": {"reasoningLevel": s.reasoning_level}})
    });
    if cancel.is_cancelled() {
        bail!("Cancelled");
    }
    let display = call["function"]["name"]
        .as_str()
        .filter(|n| n.starts_with("mcp__"))
        .and_then(|n| tools.mcp_tool(&sink.session_id, n)?.display);
    sink.send(Event::ToolStart {
        call: call.clone(),
        display,
    })
    .await?;
    let mut tool_name = call["function"]["name"]
        .as_str()
        .context("Tool name missing")?
        .to_owned();
    let mut call = call;
    // TS call-runner：工具按本会话注册表查找（含别名，执行用规范名）；查不到即 `Tool not found`，不执行。
    if let Err(content) = resolve_registered(context_registered, &mut tool_name) {
        let output = crate::contract::ToolOutput {
            failed: true,
            ..crate::contract::ToolOutput::text(content)
        };
        return Ok((
            call["id"].as_str().context("Tool id missing")?.into(),
            tool_name,
            output,
            true,
            false,
        ));
    }
    // ToolSearch / DeferredToolCall 控制面：必须在注册表查找与入参校验之前处理。
    if let Some(search) = tool_search {
        if tool_name == TOOL_SEARCH_NAME {
            let args: Value =
                serde_json::from_str(call["function"]["arguments"].as_str().unwrap_or("{}"))
                    .unwrap_or(Value::Null);
            let query = args["query"].as_str().unwrap_or_default();
            let max_results = args["max_results"]
                .as_u64()
                .map(|value| value as usize)
                .unwrap_or(escode_cli_domain::tool_search_catalog::TOOL_SEARCH_DEFAULT_RESULTS);
            let matches = search_tools(query, &search.catalog, max_results);
            if let Ok(mut discovered) = search.discovered.lock() {
                discovered.extend(matches.iter().cloned());
            }
            tools.record_tool_search(&sink.session_id, matches.clone());
            let total_deferred_tools = search
                .catalog
                .iter()
                .filter(|candidate| candidate.deferred)
                .count();
            let data = serde_json::json!({
                "matches": matches,
                "query": query,
                "total_deferred_tools": total_deferred_tools,
            });
            let content = if matches.is_empty() {
                "No matching deferred tools found".to_string()
            } else {
                data.to_string()
            };
            let mut output = crate::contract::ToolOutput::new(content, data);
            if search.mode == escode_cli_domain::tool_search_catalog::ResolvedToolSearchMode::Native
                && !matches.is_empty()
            {
                // Native：工具结果以 tool_reference part 回给模型，由 provider 展开 schema。
                output.media = tool_reference_blocks(&matches);
            }
            return Ok((
                call["id"].as_str().context("Tool id missing")?.into(),
                tool_name,
                output,
                false,
                false,
            ));
        }
        if tool_name == DEFERRED_TOOL_CALL_NAME {
            let args: Value =
                serde_json::from_str(call["function"]["arguments"].as_str().unwrap_or("{}"))
                    .unwrap_or(Value::Null);
            let target = args["name"].as_str().unwrap_or_default();
            let input = args.get("input").cloned().unwrap_or(Value::Null);
            let resolved = {
                let discovered = search
                    .discovered
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Tool search state unavailable"))?;
                resolve_deferred_call(target, &input, &search.catalog, &discovered, search.mode)
            };
            match resolved {
                Ok((resolved, resolved_input)) => {
                    call["function"]["name"] = resolved.clone().into();
                    call["function"]["arguments"] = resolved_input.to_string().into();
                    tool_name = resolved;
                }
                Err(message) => {
                    let output = crate::contract::ToolOutput {
                        failed: true,
                        ..crate::contract::ToolOutput::text(format!(
                            "<tool_use_error>{message}</tool_use_error>"
                        ))
                    };
                    return Ok((
                        call["id"].as_str().context("Tool id missing")?.into(),
                        tool_name,
                        output,
                        true,
                        false,
                    ));
                }
            }
        }
    } else if tool_name == TOOL_SEARCH_NAME || tool_name == DEFERRED_TOOL_CALL_NAME {
        let output = crate::contract::ToolOutput {
            failed: true,
            ..crate::contract::ToolOutput::text(
                "<tool_use_error>Tool search is not enabled for this request.</tool_use_error>"
                    .to_string(),
            )
        };
        return Ok((
            call["id"].as_str().context("Tool id missing")?.into(),
            tool_name,
            output,
            true,
            false,
        ));
    }
    call["function"]["name"] = tool_name.clone().into();
    let name = tool_name.as_str();
    // TS validateInitialModelToolInput：入参先按发给模型的定义 parameters 校验，失败时不请求权限、不调用工具，
    // 把问题回传模型（docs/specs/rust-tool-input-validation.md；Rust 之前 MCP 原样透传、内置工具用各自文案）。
    match checked_input(definitions, name, &call) {
        Err(content) => {
            let output = crate::contract::ToolOutput {
                failed: true,
                ..crate::contract::ToolOutput::text(content)
            };
            return Ok((
                call["id"].as_str().context("Tool id missing")?.into(),
                tool_name,
                output,
                true,
                false,
            ));
        }
        Ok(Some(arguments)) => call["function"]["arguments"] = arguments.into(),
        Ok(None) => {}
    }
    // TS validateInput → resolveInput → prepareApproval：失败直接交回模型、不请求权限；成功则把入参换成
    // 将要执行的事实（确认窗与 handler 读同一份），并带上审批门的结论。
    let approval_proceed = match prepare(
        tools,
        &sink.session_id,
        name,
        &mut call,
        workflow_skill_loaded,
    )
    .await?
    {
        Some(output) => {
            return Ok((
                call["id"].as_str().context("Tool id missing")?.into(),
                tool_name,
                output,
                true,
                false,
            ));
        }
        None => {
            call.as_object_mut()
                .and_then(|fields| fields.remove("_escode_approval_proceed"))
                == Some(Value::Bool(true))
        }
    };
    // PreToolUse hooks（docs/specs/rust-hooks.md）：预处理之后、权限之前。
    let pre = match super::tool_hooks::pre_tool_use(tools, turn, &mut call, definitions).await {
        Ok(pre) => pre,
        Err(output) => {
            let denied = output.control.denied;
            return Ok((
                call["id"].as_str().context("Tool id missing")?.into(),
                tool_name,
                output,
                true,
                denied,
            ));
        }
    };
    // 判定统一由会话 owner 完成（模式、规则与确认交互都在那里）；这里只消费结论。
    let outcome = {
        let (reply, receipt) = oneshot::channel();
        sink.send(Event::Permission {
            call: call.clone(),
            memory_root: memory_root.map(str::to_owned),
            approval_proceed,
            hook: pre.permission.clone(),
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
        // Bash 带上发起调用的 id：后台完成通知的 `<tool-use-id>`（TS tracker 持有 toolCall）。
        match serde_json::from_str::<Value>(call["function"]["arguments"].as_str().unwrap_or(""))
            .map(|mut args| {
                if name == "Bash" && args.is_object() {
                    args[crate::domain::background::TOOL_CALL_ID_ARG] = call["id"].clone();
                }
                args
            }) {
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
            Ok(args) if crate::domain::wiki::tools::is_wiki_tool(name) => {
                super::wiki_context::execute(turn.wiki.as_deref(), name, &args, &sink.session_id)
                    .await
            }
            Ok(_) if name == "ListModels" => super::model_catalog_tool::execute(
                turn.model_catalog.as_deref(),
                selection.as_ref(),
            ),
            Ok(args)
                if let Some(output) = tools
                    .execute_workflow(
                        &sink.session_id,
                        call["id"].as_str().unwrap_or_default(),
                        name,
                        &args,
                        &selection_json,
                        &turn.mode,
                        cancel,
                    )
                    .await =>
            {
                output
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
                tools
                    .execute_mcp(name, &args, call_id, &turn.mcp_meta, sink, cancel)
                    .await
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
            crate::contract::ToolOutput::text(
                match error.downcast_ref::<crate::contract::ToolHandlerFailure>() {
                    Some(failure) => format!("<tool_use_error>{failure}</tool_use_error>"),
                    None => crate::domain::tool_failure::plain_error_text(&error.to_string()),
                },
            )
        });
    let content = super::tool_hooks::post_tool_use(tools, turn, &call, pre, content, !failed).await;
    Ok((
        call["id"].as_str().context("Tool id missing")?.into(),
        name.to_owned(),
        content,
        failed,
        denied,
    ))
}
