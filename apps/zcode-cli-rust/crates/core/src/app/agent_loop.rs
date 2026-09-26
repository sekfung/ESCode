use crate::contract::{ContextPort, Event, EventSink, ModelPort, ToolPort};
use anyhow::{Context, Result, bail};
use futures_util::{StreamExt, stream};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub(super) async fn run(
    model: &dyn ModelPort,
    tools: &dyn ToolPort,
    context: &dyn ContextPort,
    history: &mut super::context::RunContext,
    sink: &EventSink,
    cancel: &CancellationToken,
) -> Result<()> {
    if let Some(instructions) = history.manual.take() {
        return history
            .compact(model, sink, cancel, Some(&instructions))
            .await;
    }
    // 工具执行（WebFetch 的辅助模型）需要未绑定的模型，才能按注册表取最低推理档位。
    let root_model = model;
    let mut reactive_compacted = false;
    let mut continuations = 0;
    // 主会话的项目记忆：会话内首次解析后由 owner 缓存（docs/specs/rust-project-memory.md）。
    // 须先于技能初始化：后者会为提示词 Shell 名请求 user-execution 偏好，TS 在会话物化时先请求
    // runtime-materialization。
    history.memory = if history.agent_profile.is_none() {
        super::memory_run::resolve(tools, sink, cancel).await?
    } else {
        None
    };
    super::skills::initialize(tools, context, history, sink, cancel).await?;
    let skills = history.skills.clone().unwrap_or_default();
    let profile = history.agent_profile.clone();
    let mut definitions = tools.scoped_definitions(&sink.session_id, cancel).await?;
    if let Some(profile) = &profile {
        definitions.retain(|d| profile.allows(d["function"]["name"].as_str().unwrap_or("")));
    }
    if !skills.enabled {
        definitions.retain(|d| d["function"]["name"] != "Skill");
    }
    // 模型输入能力决定 Read 的 PDF 分支与 schema（rust-media-read.md）。
    let properties = model
        .bind()
        .map(|m| m.format_properties())
        .unwrap_or_else(|| model.format_properties());
    tools
        .adapt_to_model(
            &sink.session_id,
            &mut definitions,
            &properties["inputFormat"],
        )
        .await;
    // TS shouldExposeWebSearch：只有声明 provider-native 搜索的模型才看到 WebSearch（rust-websearch.md）。
    if !model.native_web_search() {
        definitions.retain(|d| d["function"]["name"] != "WebSearch");
    }
    // 本轮事实只读，移出 history 以免与工具结果写回的可变借用冲突。
    let turn_facts = std::mem::take(&mut history.turn);
    super::cron_tool::retain_visible(&mut definitions, &turn_facts, profile.is_some());
    let profiles = if definitions.iter().any(|d| d["function"]["name"] == "Agent") {
        tools.agent_profiles(cancel).await?
    } else {
        vec![]
    };
    if let Some(agent) = definitions
        .iter_mut()
        .find(|d| d["function"]["name"] == "Agent")
    {
        // 按 TS 模板内联当前 profile 目录（此前追加的自拟 catalog 段 Node 没有，见 rust-tool-surface.md）。
        agent["function"]["description"] =
            crate::domain::agent_description::render(&profiles, true).into();
    }
    let tool_tokens = definitions
        .iter()
        .map(|d| d.to_string().encode_utf16().count().div_ceil(3))
        .sum();
    let memory_index = history.memory.as_ref().and_then(|m| {
        m.index
            .as_deref()
            .and_then(|index| crate::domain::memory::index_block(&m.index_path, index))
    });
    let memory_root = history.memory.as_ref().map(|m| m.root.clone());
    let mut turns = 0;
    loop {
        if profile
            .as_ref()
            .and_then(|p| p.max_turns)
            .is_some_and(|max| turns >= max)
        {
            bail!("Subagent maxTurns reached");
        }
        turns += 1;
        // 每步冻结同一 Model，同时用于预算和请求；运行中切换不能混用旧预算和新端点。
        let bound = model.bind();
        let model = bound.as_deref().unwrap_or(model);
        let policy = model.context_policy();
        if cancel.is_cancelled() {
            bail!("Cancelled");
        }
        if continuations == 0
            && definitions
                .iter()
                .any(|d| d["function"]["name"] == "TodoWrite")
            && crate::domain::todo::should_remind(&history.messages)
        {
            let (reply, receipt) = oneshot::channel();
            sink.send(Event::TodoReminder { reply }).await?;
            let message = tokio::select! {biased;
                _=cancel.cancelled()=>bail!("Cancelled"),
                message=receipt=>message.context("Todo reminder commit failed")?,
            };
            history.push(message);
        }
        let instructions = if profile
            .as_ref()
            .is_some_and(|p| p.inject_agents_md == Some(false))
        {
            vec![]
        } else {
            context.instructions(cancel).await?
        };
        let identity = model.identity();
        let mut prefix = crate::domain::prompt::prefix(
            history.prompt_snapshot.as_ref().unwrap(),
            &instructions,
            identity
                .as_ref()
                .map(|id| (id.provider_id.as_str(), id.model_id.as_str())),
            context.desktop(),
            // 主会话暴露 Skill 且目录非空时才有该段（子代理与 TS 工作流 actor 不输出）。
            history.agent_profile.is_none() && skills.enabled && !skills.skills.is_empty(),
            history
                .memory
                .as_ref()
                .map(|m| (m.root.as_str(), memory_index.as_deref())),
        );
        if let Some(reminder) = skills.reminder() {
            prefix.push(reminder);
        }
        if let Some(profile) = &profile {
            prefix.push(json!({"role":"system","content":profile.system_prompt}));
        }
        if let Some(goal) = history.goal.as_ref().filter(|g| g.active()) {
            prefix.push(json!({"role":"user","content":format!("<system-reminder>\n{}\n</system-reminder>",goal.prompt("goalState", None))}));
        }
        let micro_threshold = if policy.automatic {
            policy.micro_threshold()
        } else {
            usize::MAX
        };
        // 记忆提取快照复用本轮最后一次请求的系统前缀与工具目录。
        if memory_root.is_some() {
            history.memory_request = Some((prefix.clone(), definitions.clone()));
        }
        let (mut messages, mut tokens) = history.projection(&prefix, tool_tokens, micro_threshold);
        if policy.automatic && tokens >= policy.threshold() {
            history.compact(model, sink, cancel, None).await?;
            (messages, tokens) = history.projection(&prefix, tool_tokens, micro_threshold);
            if tokens >= policy.threshold() {
                bail!("Context remains above budget after compaction; narrow the input");
            }
        }
        sink.send(Event::ContextUsage(json!({"usedTokens":tokens,"maxTokens":policy.window,"autoCompactThresholdTokens":if policy.automatic {Some(policy.threshold())} else {None}}))).await?;
        let output = match model.complete(messages, &definitions, sink, cancel).await {
            Err(failure)
                if policy.automatic
                    && failure.reason == "context_exceeded"
                    && !failure.output_committed
                    && !reactive_compacted
                    && crate::domain::context::split_for_summary(&history.messages, false)
                        .is_some() =>
            {
                reactive_compacted = true;
                history.compact(model, sink, cancel, None).await?;
                continue;
            }
            result => result?,
        };
        history.anchor_usage(&output.usage);
        let persist = !output.output_limit
            || output.message.as_object().is_some_and(|m| {
                ["content", "reasoning_content"].iter().any(|k| {
                    m.get(*k)
                        .and_then(Value::as_str)
                        .is_some_and(|s| !s.is_empty())
                }) || ["_zcode_responses_reasoning", "_zcode_anthropic_thinking"]
                    .iter()
                    .any(|k| {
                        m.get(*k)
                            .and_then(Value::as_array)
                            .is_some_and(|a| !a.is_empty())
                    })
            });
        if persist {
            history.push(output.message.clone());
        }
        let (committed, receipt) = oneshot::channel();
        sink.send(Event::ModelDone {
            response_id: output.response_id.clone(),
            stable: !output.output_limit && output.calls.is_empty(),
            message: persist.then_some(output.message),
            usage: output.usage,
            committed,
        })
        .await?;
        durable(receipt, cancel).await?;
        if output.output_limit {
            if continuations == 3 {
                return Err(crate::contract::ModelFailure::new(
                    "model_output_limit_exceeded",
                    true,
                )
                .into());
            }
            continuations += 1;
            history.continue_output();
            reactive_compacted = false;
            continue;
        }
        continuations = 0;
        let has_tools = !output.calls.is_empty();
        let mut stop_turn = false;
        let mut calls = output.calls.into_iter().peekable();
        while let Some(first) = calls.next() {
            let mut group = vec![first];
            if safe(tools, &sink.session_id, &group[0]) {
                while calls
                    .peek()
                    .is_some_and(|call| safe(tools, &sink.session_id, call))
                {
                    group.push(calls.next().unwrap());
                }
            }
            // 只读工具并发执行，但按原始 call 顺序持久化结果；写/Shell 不跨越该屏障。
            let mut results = stream::iter(group)
                .map(|call| {
                    super::tool_dispatch::execute(
                        tools,
                        super::tool_dispatch::ExecutionContext {
                            skills: &skills,
                            profile: profile.as_ref(),
                            profiles: &profiles,
                            selection: identity.clone(),
                            model: root_model,
                            turn: &turn_facts,
                            memory_root: memory_root.as_deref(),
                        },
                        call,
                        sink,
                        cancel,
                    )
                })
                .buffered(4);
            while let Some(result) = results.next().await {
                let (id, tool, output, failed, denied) = result?;
                stop_turn |= output.control.stop_turn;
                let content = output.content;
                // 含媒体的结果以 part 数组入史，由 model 协议层按 API 格式投影（rust-media-read.md）。
                let body = if output.media.is_empty() {
                    Value::String(content.clone())
                } else {
                    Value::Array(output.media.clone())
                };
                history.push(json!({"role":"tool","tool_call_id":id,"content":body,"_zcode_tool_failed":failed,"_zcode_tool_name":tool}));
                let (committed, receipt) = oneshot::channel();
                sink.send(Event::ToolDone {
                    id,
                    tool,
                    result: content,
                    media: output.media,
                    display: output.display,
                    failed,
                    denied,
                    committed,
                })
                .await?;
                durable(receipt, cancel).await?;
            }
        }
        // TS turnControl.stopTurnAfterResult（ExitPlanMode 被拒）：结果已提交，不再请求模型；
        // 在 StepBoundary 之前结束，排队的引导输入留给下一轮而不是并入本轮。
        if stop_turn {
            return Ok(());
        }
        let (committed, receipt) = oneshot::channel();
        sink.send(Event::StepBoundary { committed }).await?;
        let guide = tokio::select! {biased;
            _=cancel.cancelled()=>bail!("Cancelled"),
            result=receipt=>result.context("Session owner stopped before guide commit")?,
        };
        if let Some(messages) = guide {
            for message in messages {
                history.push(message);
            }
        } else if !has_tools
            && !super::goal_loop::advance(model, history, &prefix, sink, cancel).await?
        {
            return Ok(());
        }
    }
}
fn safe(tools: &dyn ToolPort, session: &str, call: &Value) -> bool {
    let name = call["function"]["name"].as_str().unwrap_or("");
    tools.concurrent_safe_scoped(session, name) && !tools.requires_permission(name)
}
pub(super) async fn durable(
    receipt: oneshot::Receiver<()>,
    cancel: &CancellationToken,
) -> Result<()> {
    tokio::select! {biased;
        _=cancel.cancelled()=>bail!("Cancelled"),
        result=receipt=>result.context("Session owner stopped before durable commit"),
    }
}
