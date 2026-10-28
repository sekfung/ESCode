//! 工具目录的本轮可见性装配：轮次事实、工作流 / cron 裁剪、ToolSearch 暴露与 Agent 目录
//! （TS registerBuiltInTools 之后的可见性过滤）。
use super::context::{RunContext, TurnFacts};
use crate::contract::{ModelPort, ToolPort};
use anyhow::Result;
use escode_cli_domain::tool_search_catalog::{self, ResolvedToolSearchMode, ToolSearchCandidate};
use serde_json::Value;
use std::collections::HashSet;
use tokio_util::sync::CancellationToken;

/// 本轮 ToolSearch 决策：目录、已发现集合与解析后的模式。
pub(super) struct ToolSearchContext {
    pub(super) mode: ResolvedToolSearchMode,
    pub(super) catalog: Vec<ToolSearchCandidate>,
    pub(super) discovered: std::sync::Mutex<HashSet<String>>,
}

pub(super) struct ToolSurface {
    pub(super) turn_facts: TurnFacts,
    pub(super) registered: Vec<Value>,
    pub(super) profiles: Vec<crate::domain::subagent::Profile>,
    pub(super) agent_listed: bool,
    pub(super) tool_search: Option<ToolSearchContext>,
}

pub(super) async fn prepare_tool_surface(
    tools: &dyn ToolPort,
    model: &dyn ModelPort,
    session: &str,
    history: &mut RunContext,
    definitions: &mut Vec<Value>,
    profile_scoped: bool,
    cancel: &CancellationToken,
) -> Result<ToolSurface> {
    // 本轮事实只读，移出 history 以免与工具结果写回的可变借用冲突。
    let turn_facts = std::mem::take(&mut history.turn);
    super::dynamic_workflow::retain_visible(definitions, &turn_facts);

    // ToolSearch：目录取自当前可见工具面；控制工具必须在 registered 快照前注入，
    // 否则 dispatch 的注册表查找会把 ToolSearch/DeferredToolCall 当成未知工具。
    let requested = turn_facts
        .tool_search_mode
        .as_deref()
        .and_then(tool_search_catalog::ToolSearchMode::parse)
        .unwrap_or(tool_search_catalog::ToolSearchMode::Off);
    let capability = model.tool_search_capability();
    let resolved = tool_search_catalog::resolve_tool_search_mode(
        requested,
        tool_search_catalog::ToolSearchCapability {
            protocol_supported: capability.protocol_supported,
            default_enabled: capability.default_enabled,
        },
        model.supports_tool_call(),
    )
    .map_err(anyhow::Error::msg)?;
    let catalog: Vec<ToolSearchCandidate> = definitions
        .iter()
        .filter_map(tool_search_catalog::candidate_from_definition)
        .collect();
    let has_deferred = catalog.iter().any(|candidate| candidate.deferred);
    let discovered: HashSet<String> = tools.tool_search_discovered(session).into_iter().collect();
    let mode = if resolved != ResolvedToolSearchMode::Off && has_deferred {
        resolved
    } else {
        ResolvedToolSearchMode::Off
    };
    if mode != ResolvedToolSearchMode::Off {
        definitions.extend(tool_search_catalog::control_definitions(mode));
    }

    // TS 注册表（registerBuiltInTools 的结果）：其后只是本轮可见性过滤，被隐藏的工具仍按注册表执行
    // （自动化轮的 Cron 写工具回自动化拒绝），注册表里没有的名字回 `Tool not found`。
    let registered = definitions.clone();
    // TS shouldExposeWebSearch：只有声明 provider-native 搜索的模型才看到 WebSearch（rust-websearch.md）。
    if !model.native_web_search() {
        definitions.retain(|d| d["function"]["name"] != "WebSearch");
    }
    super::cron_tool::retain_visible(definitions, &turn_facts, profile_scoped);
    // Local：未加载 schema 的延迟工具不进入本轮工具面；Native：保留但标记 deferLoading，
    // 由 provider 原生 tool search 决定何时展开（docs/specs/rust-tool-search.md）。
    match mode {
        ResolvedToolSearchMode::Local => definitions.retain(|definition| {
            let name = definition["function"]["name"].as_str().unwrap_or_default();
            let deferred = catalog
                .iter()
                .any(|candidate| candidate.name == name && candidate.deferred);
            !deferred || discovered.contains(name)
        }),
        ResolvedToolSearchMode::Native => {
            for definition in definitions.iter_mut() {
                let name = definition["function"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                if catalog
                    .iter()
                    .any(|candidate| candidate.name == name && candidate.deferred)
                    && !discovered.contains(&name)
                {
                    definition["function"]["deferLoading"] = true.into();
                }
            }
        }
        ResolvedToolSearchMode::Off => {}
    }

    let profiles = if definitions.iter().any(|d| d["function"]["name"] == "Agent") {
        tools.agent_profiles(cancel).await?
    } else {
        vec![]
    };
    let agent_listed = if let Some(agent) = definitions
        .iter_mut()
        .find(|d| d["function"]["name"] == "Agent")
    {
        // v3.15.1：描述是静态的，profile 目录改由每步的 agent_listing_delta 提醒通告
        // （docs/specs/rust-v3.15.1-drift.md）。
        agent["function"]["description"] = crate::domain::agent_description::description().into();
        true
    } else {
        false
    };
    let tool_search = (mode != ResolvedToolSearchMode::Off).then(|| ToolSearchContext {
        mode,
        catalog,
        discovered: std::sync::Mutex::new(discovered),
    });
    Ok(ToolSurface {
        turn_facts,
        registered,
        profiles,
        agent_listed,
        tool_search,
    })
}
