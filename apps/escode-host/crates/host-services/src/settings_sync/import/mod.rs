//! `importSelected`：按选择导入并汇总（TS `importSelected` + 四个 `import*ForAgent`）。

mod plugins_mcp;
mod skills_commands;

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{Value, json};

use super::agents::Agent;
use super::sources::{Category, SourceScope};
use crate::skills::Compare;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ImportMode {
    Copy,
    Symlink,
}

impl ImportMode {
    fn parse(value: Option<&Value>) -> Self {
        if value.and_then(Value::as_str) == Some("copy") {
            Self::Copy
        } else {
            Self::Symlink
        }
    }
}

pub struct Selection {
    pub agent: String,
    pub category: String,
    pub source_scope: Option<SourceScope>,
    pub target_scope: Option<SourceScope>,
    pub mode: ImportMode,
    pub paths: Option<BTreeSet<String>>,
}

impl Selection {
    pub fn parse(value: &Value) -> Self {
        let paths = value["skillPaths"]
            .as_array()
            .or_else(|| value["commandPaths"].as_array())
            .or_else(|| value["pluginPaths"].as_array())
            .or_else(|| value["mcpServerPaths"].as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            });
        Self {
            agent: value["agent"].as_str().unwrap_or_default().to_owned(),
            category: value["category"].as_str().unwrap_or_default().to_owned(),
            source_scope: parse_scope(value["sourceScope"].as_str()),
            target_scope: parse_scope(value["targetScope"].as_str()),
            mode: ImportMode::parse(value.get("importMode")),
            paths,
        }
    }

    pub fn source_scope_matches(&self, scope: SourceScope) -> bool {
        self.source_scope.is_none_or(|expected| expected == scope)
    }

    pub fn selected(&self, path: &str) -> bool {
        self.paths
            .as_ref()
            .is_none_or(|paths: &BTreeSet<String>| paths.contains(path))
    }
}

fn parse_scope(value: Option<&str>) -> Option<SourceScope> {
    match value {
        Some("global") => Some(SourceScope::User),
        Some("project") => Some(SourceScope::Workspace),
        _ => None,
    }
}

pub struct TaskResult {
    pub agent: String,
    pub category: String,
    pub imported: usize,
    pub skipped: usize,
    pub failed: usize,
    pub results_key: &'static str,
    pub results: Vec<Value>,
}

impl TaskResult {
    pub fn new(agent: &str, category: &str, results_key: &'static str) -> Self {
        Self {
            agent: agent.to_owned(),
            category: category.to_owned(),
            imported: 0,
            skipped: 0,
            failed: 0,
            results_key,
            results: Vec::new(),
        }
    }

    pub fn skipped_item(&mut self, value: Value, reason: &str) {
        self.skipped += 1;
        let mut value = value;
        value["status"] = Value::from("skipped");
        value["skipReason"] = Value::from(reason);
        self.results.push(value);
    }

    pub fn imported_item(&mut self, value: Value) {
        self.imported += 1;
        let mut value = value;
        value["status"] = Value::from("imported");
        self.results.push(value);
    }

    pub fn failed_item(&mut self, value: Value) {
        self.failed += 1;
        let mut value = value;
        value["status"] = Value::from("failed");
        self.results.push(value);
    }

    pub fn into_json(self) -> Value {
        let status = if self.failed > 0 {
            "failed"
        } else if self.imported > 0 {
            "success"
        } else {
            "skipped"
        };
        json!({
            "agent": self.agent,
            "category": self.category,
            "status": status,
            "importedCount": self.imported,
            "skippedCount": self.skipped,
            "failedCount": self.failed,
            self.results_key: self.results,
        })
    }

    fn skipped_task(self) -> Value {
        json!({
            "agent": self.agent,
            "category": self.category,
            "status": "skipped",
            "importedCount": 0,
            "skippedCount": 1,
            "failedCount": 0,
        })
    }
}

pub fn import_selected(
    home: &Path,
    workspace: Option<&Path>,
    request: &Value,
    compare: &Compare,
) -> Value {
    let mut task_results = Vec::new();
    let selections = request["selections"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for value in selections {
        let selection = Selection::parse(&value);
        let category = Category::from_str(&selection.category);
        let task = match category {
            Some(Category::Skills) => {
                skills_commands::import_skills(home, workspace, &selection, compare)
            }
            Some(Category::Commands) => {
                skills_commands::import_commands(home, workspace, &selection, compare)
            }
            Some(Category::Plugins) => {
                plugins_mcp::import_plugins(home, workspace, &selection, compare)
            }
            Some(Category::McpServers) => plugins_mcp::import_mcp(home, workspace, &selection),
            _ => {
                let mut task = TaskResult::new(&selection.agent, &selection.category, "results");
                task.skipped += 1;
                task_results.push(task.skipped_task());
                continue;
            }
        };
        task_results.push(task.into_json());
    }
    let success = task_results
        .iter()
        .map(|task| task["importedCount"].as_u64().unwrap_or(0))
        .sum::<u64>();
    let skipped = task_results
        .iter()
        .map(|task| task["skippedCount"].as_u64().unwrap_or(0))
        .sum::<u64>();
    let failed = task_results
        .iter()
        .map(|task| task["failedCount"].as_u64().unwrap_or(0))
        .sum::<u64>();
    json!({
        "successCount": success,
        "skippedCount": skipped,
        "failedCount": failed,
        "taskResults": task_results,
    })
}

/// 候选 agent 过滤（按 TS 字符串比较，未知 agent 自然无候选）。
pub(super) fn agent_matches(agent: Agent, selection: &Selection) -> bool {
    agent.as_str() == selection.agent
}

/// skill 候选的公共结果项（name/path/sourceScope/version）。
pub(super) fn skill_item(candidate: &super::candidates::SkillCandidate) -> Value {
    let mut item = json!({
        "name": candidate.name,
        "path": candidate.source_path.to_string_lossy(),
        "sourceScope": candidate.source_root_scope.as_str(),
    });
    if let Some(version) = &candidate.version {
        item["version"] = Value::from(version.clone());
    }
    item
}

pub(super) fn command_item(candidate: &super::candidates::CommandCandidate) -> Value {
    json!({
        "name": candidate.name,
        "path": candidate.source_path.to_string_lossy(),
        "sourceScope": candidate.source_root_scope.as_str(),
    })
}

pub(super) fn plugin_item(candidate: &super::candidates::PluginCandidate) -> Value {
    let mut item = json!({
        "name": candidate.name,
        "path": candidate.source_path.to_string_lossy(),
        "sourceScope": candidate.source_root_scope.as_str(),
    });
    if let Some(version) = &candidate.version {
        item["version"] = Value::from(version.clone());
    }
    item
}

pub(super) fn mcp_item(candidate: &super::candidates::McpCandidate) -> Value {
    json!({
        "name": candidate.name,
        "path": candidate.source_path,
        "sourceScope": candidate.source_root_scope.as_str(),
    })
}

#[cfg(test)]
#[path = "import_tests.rs"]
mod tests;
