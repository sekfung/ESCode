//! `sourceRoots` 汇总（TS `build*SourceRootSummaries`）。

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use super::super::candidates::{CommandCandidate, McpCandidate, PluginCandidate, SkillCandidate};
use super::super::sources::SourceScope;
use super::skip::{SkipCaches, command_skip, mcp_skip, plugin_skip, skill_skip};
use crate::skills::Compare;

fn group_by_root<'a, T>(
    candidates: &[&'a T],
    scope: impl Fn(&T) -> SourceScope,
    path: impl Fn(&T) -> PathBuf,
) -> Vec<(SourceScope, PathBuf, Vec<&'a T>)> {
    let mut groups: Vec<(SourceScope, PathBuf, Vec<&'a T>)> = Vec::new();
    for candidate in candidates {
        let candidate_scope = scope(candidate);
        let candidate_path = path(candidate);
        match groups
            .iter_mut()
            .find(|(scope, path, _)| *scope == candidate_scope && *path == candidate_path)
        {
            Some((_, _, group)) => group.push(candidate),
            None => groups.push((candidate_scope, candidate_path, vec![candidate])),
        }
    }
    groups
}

fn maybe_version(value: &Option<String>) -> Value {
    value.clone().map(Value::from).unwrap_or(Value::Null)
}

fn with_optional(object: &mut Map<String, Value>, key: &str, value: Value) {
    if !value.is_null() {
        object.insert(key.to_owned(), value);
    }
}

fn sort_items(items: &mut [Value], compare: &Compare) {
    items.sort_by(|left, right| {
        compare(
            left["name"].as_str().unwrap_or_default(),
            right["name"].as_str().unwrap_or_default(),
        )
    });
}

fn root_summary(
    scope: SourceScope,
    path: &Path,
    discovered: usize,
    importable: usize,
    category: &str,
    items: Vec<Value>,
) -> Value {
    let mut object = Map::new();
    object.insert("scope".to_owned(), Value::from(scope.as_str()));
    object.insert(
        "path".to_owned(),
        Value::from(path.to_string_lossy().into_owned()),
    );
    object.insert("discoveredCount".to_owned(), Value::from(discovered));
    object.insert("importableCount".to_owned(), Value::from(importable));
    object.insert(
        "skippedCount".to_owned(),
        Value::from(discovered - importable),
    );
    object.insert(category.to_owned(), Value::Array(items));
    Value::Object(object)
}

fn sort_summaries(mut summaries: Vec<Value>, compare: &Compare) -> Vec<Value> {
    summaries.sort_by(|left, right| {
        let by_scope = compare(
            left["scope"].as_str().unwrap_or_default(),
            right["scope"].as_str().unwrap_or_default(),
        );
        if by_scope != std::cmp::Ordering::Equal {
            return by_scope;
        }
        compare(
            left["path"].as_str().unwrap_or_default(),
            right["path"].as_str().unwrap_or_default(),
        )
    });
    summaries
}

pub fn skill_summaries(
    candidates: &[&SkillCandidate],
    caches: &mut SkipCaches,
    compare: &Compare,
) -> Vec<Value> {
    let groups = group_by_root(
        candidates,
        |c| c.source_root_scope,
        |c| c.source_root_path.clone(),
    );
    let mut summaries = Vec::new();
    for (scope, path, group) in groups {
        let mut items = Vec::new();
        let mut importable = 0;
        for candidate in group {
            let skip = skill_skip(
                &candidate.target_root,
                &candidate.target_path,
                &candidate.name_key,
                caches,
            );
            if skip.is_none() {
                importable += 1;
            }
            let mut item = Map::new();
            item.insert("name".to_owned(), Value::from(candidate.name.clone()));
            item.insert(
                "path".to_owned(),
                Value::from(candidate.source_path.to_string_lossy().into_owned()),
            );
            item.insert("importable".to_owned(), Value::Bool(skip.is_none()));
            if let Some(reason) = skip {
                item.insert("skipReason".to_owned(), Value::from(reason));
            }
            with_optional(&mut item, "version", maybe_version(&candidate.version));
            items.push(Value::Object(item));
        }
        sort_items(&mut items, compare);
        let discovered = items.len();
        summaries.push(root_summary(
            scope, &path, discovered, importable, "skills", items,
        ));
    }
    sort_summaries(summaries, compare)
}

pub fn command_summaries(
    candidates: &[&CommandCandidate],
    caches: &mut SkipCaches,
    compare: &Compare,
) -> Vec<Value> {
    let groups = group_by_root(
        candidates,
        |c| c.source_root_scope,
        |c| c.source_root_path.clone(),
    );
    let mut summaries = Vec::new();
    for (scope, path, group) in groups {
        let mut items = Vec::new();
        let mut importable = 0;
        for candidate in group {
            let skip = command_skip(
                &candidate.target_root,
                &candidate.target_path,
                &candidate.name_key,
                caches,
            );
            if skip.is_none() {
                importable += 1;
            }
            let mut item = Map::new();
            item.insert("name".to_owned(), Value::from(candidate.name.clone()));
            item.insert(
                "path".to_owned(),
                Value::from(candidate.source_path.to_string_lossy().into_owned()),
            );
            item.insert("importable".to_owned(), Value::Bool(skip.is_none()));
            if let Some(reason) = skip {
                item.insert("skipReason".to_owned(), Value::from(reason));
            }
            with_optional(
                &mut item,
                "description",
                candidate
                    .description
                    .clone()
                    .map(Value::from)
                    .unwrap_or(Value::Null),
            );
            with_optional(
                &mut item,
                "argumentHint",
                candidate
                    .argument_hint
                    .clone()
                    .map(Value::from)
                    .unwrap_or(Value::Null),
            );
            items.push(Value::Object(item));
        }
        sort_items(&mut items, compare);
        let discovered = items.len();
        summaries.push(root_summary(
            scope, &path, discovered, importable, "commands", items,
        ));
    }
    sort_summaries(summaries, compare)
}

pub fn plugin_summaries(
    candidates: &[&PluginCandidate],
    home: &Path,
    workspace: Option<&Path>,
    caches: &mut SkipCaches,
    compare: &Compare,
) -> Vec<Value> {
    let groups = group_by_root(
        candidates,
        |c| c.source_root_scope,
        |c| c.source_root_path.clone(),
    );
    let mut summaries = Vec::new();
    for (scope, path, group) in groups {
        let mut items = Vec::new();
        let mut importable = 0;
        for candidate in group {
            let config =
                super::super::candidates::config_path(home, workspace, candidate.source_root_scope);
            let skip = plugin_skip(
                &candidate.target_path,
                &candidate.id,
                config.as_deref(),
                caches,
            );
            if skip.is_none() {
                importable += 1;
            }
            let mut item = Map::new();
            item.insert("name".to_owned(), Value::from(candidate.name.clone()));
            item.insert(
                "path".to_owned(),
                Value::from(candidate.source_path.to_string_lossy().into_owned()),
            );
            item.insert("importable".to_owned(), Value::Bool(skip.is_none()));
            if let Some(reason) = skip {
                item.insert("skipReason".to_owned(), Value::from(reason));
            }
            with_optional(&mut item, "version", maybe_version(&candidate.version));
            items.push(Value::Object(item));
        }
        sort_items(&mut items, compare);
        let discovered = items.len();
        summaries.push(root_summary(
            scope, &path, discovered, importable, "plugins", items,
        ));
    }
    sort_summaries(summaries, compare)
}

pub fn mcp_summaries(
    candidates: &[&McpCandidate],
    home: &Path,
    workspace: Option<&Path>,
    caches: &mut SkipCaches,
    compare: &Compare,
) -> Vec<Value> {
    let groups = group_by_root(
        candidates,
        |c| c.source_root_scope,
        |c| c.source_root_path.clone(),
    );
    let mut summaries = Vec::new();
    for (scope, path, group) in groups {
        let mut items = Vec::new();
        let mut importable = 0;
        for candidate in group {
            let skip = mcp_skip(
                candidate.source_root_scope,
                home,
                workspace,
                &candidate.name_key,
                caches,
            );
            if skip.is_none() {
                importable += 1;
            }
            let mut item = Map::new();
            item.insert("name".to_owned(), Value::from(candidate.name.clone()));
            item.insert(
                "path".to_owned(),
                Value::from(candidate.source_path.clone()),
            );
            item.insert("importable".to_owned(), Value::Bool(skip.is_none()));
            if let Some(reason) = skip {
                item.insert("skipReason".to_owned(), Value::from(reason));
            }
            items.push(Value::Object(item));
        }
        sort_items(&mut items, compare);
        let discovered = items.len();
        summaries.push(root_summary(
            scope,
            &path,
            discovered,
            importable,
            "mcpServers",
            items,
        ));
    }
    sort_summaries(summaries, compare)
}
