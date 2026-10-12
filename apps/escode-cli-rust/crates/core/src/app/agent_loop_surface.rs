//! 工具目录的本轮可见性装配：轮次事实、off-peak / 工作流 / cron 裁剪与 Agent 目录
//! （TS registerBuiltInTools 之后的可见性过滤）。
use super::context::{RunContext, TurnFacts};
use crate::contract::{ModelPort, ToolPort};
use anyhow::Result;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

pub(super) struct ToolSurface {
    pub(super) turn_facts: TurnFacts,
    pub(super) registered: Vec<Value>,
    pub(super) profiles: Vec<crate::domain::subagent::Profile>,
    pub(super) agent_listed: bool,
}

pub(super) async fn prepare_tool_surface(
    tools: &dyn ToolPort,
    model: &dyn ModelPort,
    history: &mut RunContext,
    definitions: &mut Vec<Value>,
    profile_scoped: bool,
    cancel: &CancellationToken,
) -> Result<ToolSurface> {
    // 本轮事实只读，移出 history 以免与工具结果写回的可变借用冲突。
    let turn_facts = std::mem::take(&mut history.turn);
    super::off_peak::retain_visible(definitions, &turn_facts, profile_scoped);
    super::dynamic_workflow::retain_visible(definitions, &turn_facts);
    // TS 注册表（registerBuiltInTools 的结果）：其后只是本轮可见性过滤，被隐藏的工具仍按注册表执行
    // （自动化轮的 Cron 写工具回自动化拒绝），注册表里没有的名字回 `Tool not found`。
    let registered = definitions.clone();
    super::off_peak::hide_restricted(definitions, &turn_facts);
    // TS shouldExposeWebSearch：只有声明 provider-native 搜索的模型才看到 WebSearch（rust-websearch.md）。
    if !model.native_web_search() {
        definitions.retain(|d| d["function"]["name"] != "WebSearch");
    }
    super::cron_tool::retain_visible(definitions, &turn_facts, profile_scoped);
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
        agent["function"]["description"] =
            crate::domain::agent_description::description().into();
        true
    } else {
        false
    };
    Ok(ToolSurface {
        turn_facts,
        registered,
        profiles,
        agent_listed,
    })
}
