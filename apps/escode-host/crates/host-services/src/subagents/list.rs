//! `subagents.list` 编排（TS `subagentsService.ts:620-690`）。
//!
//! 顺序即语义：内置 → 用户 → 工作区 → 插件运行时集，再按 `name` 做优先级合并（后者覆盖前者），
//! 最后套用禁用态。`mode === "settingsUserOnly"` 时**不**做插件发现、也不做优先级合并。

use std::cmp::Ordering;
use std::path::Path;

use serde_json::{Value, json};

use super::discovery_file::{
    apply_runtime_precedence, attach_enabled_state, built_in_agents, capability,
    discover_file_agents,
};
use super::discovery_plugin::discover_plugin_agents;
use super::state;
use super::storage;

type Compare = dyn Fn(&str, &str) -> Ordering + Send + Sync;

pub struct ListParams<'a> {
    pub workspace_path: &'a str,
    /// 缺省 `allRuntimeScopes`；`settingsUserOnly` 只回内置 + 用户命令。
    pub mode: Option<&'a str>,
}

pub async fn list(home: &Path, params: ListParams<'_>, compare: &Compare) -> Result<Value, String> {
    let state = state::read_state(&storage::state_file(home).await).await?;
    let mode = params.mode.unwrap_or("allRuntimeScopes");
    let mut diagnostics: Vec<Value> = Vec::new();

    let built_in = built_in_agents(
        state
            .get("builtInModelSelectionOverrides")
            .unwrap_or(&Value::Null),
    );
    let file_agents = discover_file_agents(
        home,
        params.workspace_path,
        true,
        mode == "allRuntimeScopes",
        &mut diagnostics,
        compare,
    )
    .await;

    let name_of = |agent: &Value| agent["name"].as_str().unwrap_or_default().to_owned();
    let mut user_agents: Vec<Value> = file_agents
        .iter()
        .filter(|agent| agent["scope"].as_str() == Some("user"))
        .cloned()
        .collect();
    user_agents.sort_by(|left, right| compare(&name_of(left), &name_of(right)));
    let mut workspace_agents: Vec<Value> = file_agents
        .into_iter()
        .filter(|agent| agent["scope"].as_str() == Some("workspace"))
        .collect();
    workspace_agents.sort_by(|left, right| compare(&name_of(left), &name_of(right)));

    // 保留名：内置 + 已发现的用户/工作区 name（插件的裸名不能撞上它们）。
    let reserved: Vec<String> = built_in
        .iter()
        .chain(user_agents.iter())
        .chain(workspace_agents.iter())
        .map(name_of)
        .collect();
    let plugin = discover_plugin_agents(home, &state, &reserved, compare).await;
    diagnostics.extend(plugin.diagnostics);

    let discovered = if mode == "settingsUserOnly" {
        let mut agents = built_in.clone();
        agents.extend(user_agents.clone());
        agents
    } else {
        let mut agents = built_in.clone();
        agents.extend(user_agents.clone());
        agents.extend(workspace_agents);
        agents.extend(plugin.runtime_agents);
        apply_runtime_precedence(agents)
    };
    let agents = attach_enabled_state(discovered, &state);
    let user_projection: Vec<Value> = agents
        .iter()
        .filter(|agent| agent["source"].as_str() == Some("user"))
        .cloned()
        .collect();

    Ok(json!({
        "agents": agents,
        "userAgents": user_projection,
        "pluginAgents": attach_enabled_state(plugin.profiles, &state),
        "capability": capability(),
        "diagnostics": diagnostics,
    }))
}

#[cfg(test)]
#[path = "list_tests.rs"]
mod tests;
