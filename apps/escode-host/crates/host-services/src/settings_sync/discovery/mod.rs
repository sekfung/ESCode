//! `detect`：按类别发现外部 agent 的设置（TS `build*Discovery`）。

pub mod skip;
mod summary;

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::agents::Agent;
use super::candidates::{
    CommandCandidate, McpCandidate, PluginCandidate, SkillCandidate, collect_command_candidates,
    collect_mcp_candidates, collect_plugin_candidates, collect_skill_candidates,
};
use super::sources::{COMMAND_SOURCES, Category, MCP_SOURCES, PLUGIN_SOURCES, SKILL_SOURCES};
use crate::skills::Compare;
use skip::{SkipCaches, command_skip, mcp_skip, plugin_skip, skill_skip};

pub fn detect(home: &Path, workspace: Option<&Path>, request: &Value, compare: &Compare) -> Value {
    let manual = request["intent"].as_str() == Some("manualImport");
    let categories: Vec<&str> = request["categories"]
        .as_array()
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let category = if !manual {
        None
    } else if categories.contains(&"skills") {
        Some(Category::Skills)
    } else if categories.contains(&"commands") {
        Some(Category::Commands)
    } else if categories.contains(&"plugins") {
        Some(Category::Plugins)
    } else if categories.contains(&"mcpServers") {
        Some(Category::McpServers)
    } else {
        None
    };
    match category {
        Some(Category::Skills) => skills_discovery(home, workspace, compare),
        Some(Category::Commands) => commands_discovery(home, workspace, compare),
        Some(Category::Plugins) => plugins_discovery(home, workspace, compare),
        Some(Category::McpServers) => mcp_discovery(home, workspace, compare),
        _ => json!({"agents": []}),
    }
}

fn skills_discovery(home: &Path, workspace: Option<&Path>, compare: &Compare) -> Value {
    let candidates = collect_skill_candidates(home, workspace, compare);
    let mut summary_cache = SkipCaches::default();
    let mut agents = Vec::new();
    for agent in agents_of(SKILL_SOURCES) {
        let group: Vec<&SkillCandidate> = candidates
            .iter()
            .filter(|candidate| candidate.agent == agent)
            .collect();
        if group.is_empty() {
            continue;
        }
        let source_paths = unique_paths(
            group.iter().map(|candidate| &candidate.source_root_path),
            compare,
        );
        let roots = summary::skill_summaries(&group, &mut summary_cache, compare);
        let mut import_cache = SkipCaches::default();
        let importable = group
            .iter()
            .filter(|candidate| {
                skill_skip(
                    &candidate.target_root,
                    &candidate.target_path,
                    &candidate.name_key,
                    &mut import_cache,
                )
                .is_none()
            })
            .count();
        agents.push(agent_summary(
            agent,
            &source_paths,
            group.len(),
            importable,
            importable > 0,
            "skills",
            json!(roots),
        ));
    }
    json!({"agents": agents})
}

fn commands_discovery(home: &Path, workspace: Option<&Path>, compare: &Compare) -> Value {
    let candidates = collect_command_candidates(home, workspace, compare);
    let mut summary_cache = SkipCaches::default();
    let mut agents = Vec::new();
    for agent in agents_of(COMMAND_SOURCES) {
        let group: Vec<&CommandCandidate> = candidates
            .iter()
            .filter(|candidate| candidate.agent == agent)
            .collect();
        if group.is_empty() {
            continue;
        }
        let source_paths = unique_paths(
            group.iter().map(|candidate| &candidate.source_root_path),
            compare,
        );
        let roots = summary::command_summaries(&group, &mut summary_cache, compare);
        let mut import_cache = SkipCaches::default();
        let importable = group
            .iter()
            .filter(|candidate| {
                command_skip(
                    &candidate.target_root,
                    &candidate.target_path,
                    &candidate.name_key,
                    &mut import_cache,
                )
                .is_none()
            })
            .count();
        agents.push(agent_summary(
            agent,
            &source_paths,
            group.len(),
            importable,
            false,
            "commands",
            json!(roots),
        ));
    }
    json!({"agents": agents})
}

fn plugins_discovery(home: &Path, workspace: Option<&Path>, compare: &Compare) -> Value {
    let candidates = collect_plugin_candidates(home, workspace, compare);
    let mut summary_cache = SkipCaches::default();
    let mut agents = Vec::new();
    for agent in agents_of(PLUGIN_SOURCES) {
        let group: Vec<&PluginCandidate> = candidates
            .iter()
            .filter(|candidate| candidate.agent == agent)
            .collect();
        if group.is_empty() {
            continue;
        }
        let source_paths = unique_paths(
            group.iter().map(|candidate| &candidate.source_root_path),
            compare,
        );
        let roots = summary::plugin_summaries(&group, home, workspace, &mut summary_cache, compare);
        let mut import_cache = SkipCaches::default();
        let importable = group
            .iter()
            .filter(|candidate| {
                let config =
                    super::candidates::config_path(home, workspace, candidate.source_root_scope);
                plugin_skip(
                    &candidate.target_path,
                    &candidate.id,
                    config.as_deref(),
                    &mut import_cache,
                )
                .is_none()
            })
            .count();
        agents.push(agent_summary(
            agent,
            &source_paths,
            group.len(),
            importable,
            false,
            "plugins",
            json!(roots),
        ));
    }
    json!({"agents": agents})
}

fn mcp_discovery(home: &Path, workspace: Option<&Path>, compare: &Compare) -> Value {
    let candidates = collect_mcp_candidates(home, workspace);
    let mut summary_cache = SkipCaches::default();
    let mut agents = Vec::new();
    for agent in agents_of_mcp() {
        let group: Vec<&McpCandidate> = candidates
            .iter()
            .filter(|candidate| candidate.agent == agent)
            .collect();
        if group.is_empty() {
            continue;
        }
        let source_paths = unique_paths(
            group.iter().map(|candidate| &candidate.source_root_path),
            compare,
        );
        let roots = summary::mcp_summaries(&group, home, workspace, &mut summary_cache, compare);
        let mut import_cache = SkipCaches::default();
        let importable = group
            .iter()
            .filter(|candidate| {
                mcp_skip(
                    candidate.source_root_scope,
                    home,
                    workspace,
                    &candidate.name_key,
                    &mut import_cache,
                )
                .is_none()
            })
            .count();
        agents.push(agent_summary(
            agent,
            &source_paths,
            group.len(),
            importable,
            false,
            "mcpServers",
            json!(roots),
        ));
    }
    json!({"agents": agents})
}

fn agents_of(sources: &[super::sources::PathSource]) -> Vec<Agent> {
    let mut agents = Vec::new();
    for source in sources {
        if !agents.contains(&source.agent) {
            agents.push(source.agent);
        }
    }
    agents
}

fn agents_of_mcp() -> Vec<Agent> {
    let mut agents = Vec::new();
    for source in MCP_SOURCES {
        if !agents.contains(&source.agent) {
            agents.push(source.agent);
        }
    }
    agents
}

fn unique_paths<'a>(paths: impl Iterator<Item = &'a PathBuf>, compare: &Compare) -> Vec<String> {
    let mut unique: Vec<String> = paths
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    unique.sort_by(|left, right| compare(left, right));
    unique.dedup();
    unique
}

fn agent_summary(
    agent: Agent,
    source_paths: &[String],
    discovered: usize,
    importable: usize,
    selected_by_default: bool,
    category: &str,
    source_roots: Value,
) -> Value {
    json!({
        "agent": agent.as_str(),
        "discovered": true,
        "categories": [{
            "category": category,
            "discoveredCount": discovered,
            "importableCount": importable,
            "skippedCount": discovered - importable,
            "sourcePaths": source_paths,
            "sourceRoots": source_roots,
            "selectedByDefault": selected_by_default,
        }],
    })
}

#[cfg(test)]
#[path = "discovery_tests.rs"]
mod tests;
