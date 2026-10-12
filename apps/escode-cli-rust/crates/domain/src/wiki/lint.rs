//! Wiki lint（spec §6 治理）：schema、重复 ID、断链、空正文、导入条目缺来源、过期提案。
use super::entry::{Entry, Origin, Status, validate};
use serde::Serialize;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LintIssue {
    pub severity: &'static str,
    pub code: &'static str,
    pub id: String,
    pub message: String,
}

/// 提案超过该天数未处理视为积压。
pub const STALE_PROPOSAL_DAYS: i64 = 30;

/// `stores`: (store_project, entries)。`invalid_files`: 无法解析的条目文件（路径, 错误）。
/// `age_days(updated_at)` 返回条目距今天数（时间计算在适配器）。
pub fn lint(
    stores: &[(Option<&str>, &[Entry])],
    invalid_files: &[(String, String)],
    age_days: &dyn Fn(&str) -> Option<i64>,
) -> Vec<LintIssue> {
    let mut issues = vec![];
    for (path, error) in invalid_files {
        issues.push(issue("error", "unparseable_entry", path, error.clone()));
    }
    let mut seen: HashMap<&str, usize> = HashMap::new();
    let all: Vec<&Entry> = stores.iter().flat_map(|(_, e)| e.iter()).collect();
    for (project, entries) in stores {
        for entry in *entries {
            for problem in validate(&entry.meta, *project) {
                issues.push(issue("error", "schema", &entry.meta.id, problem));
            }
            *seen.entry(entry.meta.id.as_str()).or_default() += 1;
            if entry.body.trim().is_empty() {
                issues.push(issue(
                    "warning",
                    "empty_body",
                    &entry.meta.id,
                    "entry has no body".into(),
                ));
            }
            if entry.meta.origin == Origin::Imported && entry.meta.sources.is_empty() {
                issues.push(issue(
                    "error",
                    "missing_source",
                    &entry.meta.id,
                    "imported entry has no source reference".into(),
                ));
            }
            for target in entry
                .meta
                .related
                .iter()
                .chain(&entry.meta.supersedes)
                .chain(&entry.meta.superseded_by)
            {
                if !all.iter().any(|e| &e.meta.id == target) {
                    issues.push(issue(
                        "warning",
                        "broken_reference",
                        &entry.meta.id,
                        format!("references missing entry '{target}'"),
                    ));
                }
            }
            if entry.meta.status == Status::Superseded && entry.meta.superseded_by.is_none() {
                issues.push(issue(
                    "warning",
                    "superseded_without_target",
                    &entry.meta.id,
                    "superseded entry should name superseded_by".into(),
                ));
            }
            if entry.meta.status == Status::Proposal
                && age_days(&entry.meta.updated_at).is_some_and(|d| d > STALE_PROPOSAL_DAYS)
            {
                issues.push(issue(
                    "info",
                    "stale_proposal",
                    &entry.meta.id,
                    format!("proposal pending review for more than {STALE_PROPOSAL_DAYS} days"),
                ));
            }
        }
    }
    for (id, count) in seen {
        if count > 1 {
            issues.push(issue(
                "warning",
                "duplicate_id",
                id,
                format!("id is used by {count} entries across scopes"),
            ));
        }
    }
    issues.sort_by(|a, b| a.id.cmp(&b.id).then(a.code.cmp(b.code)));
    issues
}

fn issue(severity: &'static str, code: &'static str, id: &str, message: String) -> LintIssue {
    LintIssue {
        severity,
        code,
        id: id.to_owned(),
        message,
    }
}
