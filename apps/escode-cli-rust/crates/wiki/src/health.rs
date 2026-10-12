//! 只读诊断（spec §14.2）：状态、lint、来源校验（不修改）、冲突检测。
use super::service::{Snapshot, WikiService, age_days};
use anyhow::Result;
use escode_cli_domain::wiki::conflicts::{Conflict, detect};
use escode_cli_domain::wiki::entry::Entry;
use escode_cli_domain::wiki::lint::{LintIssue, lint};
use escode_cli_domain::wiki::rank::glob_match;
use escode_cli_domain::wiki::source::SourceStatus;
use escode_cli_domain::wiki::text::content_hash;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const PROJECT_FILE_LIMIT: usize = 20_000;

/// 项目内文件（相对路径），用于判断条目声明的 applies_to 是否仍有对应代码。
async fn project_files(root: &Path) -> Vec<String> {
    let mut out = vec![];
    let mut queue = vec![root.to_owned()];
    while let Some(dir) = queue.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(kind) = entry.file_type().await else {
                continue;
            };
            if kind.is_dir() {
                if !name.starts_with('.')
                    && !matches!(name.as_str(), "node_modules" | "target" | "dist")
                {
                    queue.push(entry.path());
                }
            } else if let Ok(rel) = entry.path().strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
            if out.len() >= PROJECT_FILE_LIMIT {
                return out;
            }
        }
    }
    out
}

/// 项目版本：package.json 的 version 或 Cargo.toml [package] version。
async fn project_version(root: &Path) -> Option<String> {
    if let Ok(text) = tokio::fs::read_to_string(root.join("package.json")).await
        && let Some(v) = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v["version"].as_str().map(str::to_owned))
    {
        return Some(v);
    }
    let text = tokio::fs::read_to_string(root.join("Cargo.toml"))
        .await
        .ok()?;
    let mut in_package = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
        } else if in_package
            && let Some(v) = line
                .strip_prefix("version")
                .and_then(|r| r.trim_start().strip_prefix('='))
        {
            return Some(v.trim().trim_matches('"').to_owned()).filter(|v| !v.contains('{'));
        }
    }
    None
}

fn all_entries(snapshots: &[Snapshot]) -> Vec<Entry> {
    snapshots
        .iter()
        .flat_map(|s| s.entries.values().cloned())
        .collect()
}

impl WikiService {
    pub async fn status(&self) -> Result<Value> {
        let settings = self.settings().await;
        let project = self.project().await.clone();
        let snapshots = self.open_snapshots().await?;
        let mut scopes = vec![];
        for snapshot in &snapshots {
            let mut by_status: BTreeMap<&str, usize> = BTreeMap::new();
            for entry in snapshot.entries.values() {
                *by_status.entry(entry.meta.status.as_str()).or_default() += 1;
            }
            let mut sources: BTreeMap<&str, usize> = BTreeMap::new();
            for record in snapshot.store.sources().await? {
                *sources.entry(record.status.as_str()).or_default() += 1;
            }
            scopes.push(json!({
                "scope": snapshot.store.scope.as_str(), "entries": snapshot.entries.len(), "byStatus": by_status,
                "sources": sources, "indexVersion": snapshot.index.version, "invalidFiles": snapshot.invalid.len(),
                "root": snapshot.store.root.to_string_lossy(),
            }));
        }
        let conflicts = self.conflicts().await?.len();
        Ok(json!({
            "enabled": settings.config.enabled, "autoInject": settings.config.auto_inject,
            "project": {"id": project.id, "name": project.name, "identityKind": project.kind, "root": project.root},
            "scopes": scopes, "conflicts": conflicts,
            "configIssues": settings.config.issues, "ignoredProjectKeys": settings.ignored_project_keys,
        }))
    }

    pub async fn lint(&self) -> Result<Vec<LintIssue>> {
        let snapshots = self.open_snapshots().await?;
        let project: Vec<Entry> = snapshots[0].entries.values().cloned().collect();
        let global: Vec<Entry> = snapshots[1].entries.values().cloned().collect();
        let mut invalid: Vec<(String, String)> = vec![];
        for s in &snapshots {
            invalid.extend(
                s.invalid
                    .iter()
                    .map(|(f, e)| (format!("{}/{f}", s.store.scope.as_str()), e.clone())),
            );
        }
        Ok(lint(
            &[
                (snapshots[0].store.project_id.as_deref(), &project),
                (None, &global),
            ],
            &invalid,
            &|t: &str| age_days(t),
        ))
    }

    pub async fn conflicts(&self) -> Result<Vec<Conflict>> {
        let snapshots = self.open_snapshots().await?;
        let root = PathBuf::from(&self.project().await.root);
        let files = project_files(&root).await;
        let version = project_version(&root).await;
        let entries = all_entries(&snapshots);
        Ok(detect(
            &entries,
            &|glob: &str| files.iter().any(|f| glob_match(glob, f)),
            version.as_deref(),
        ))
    }

    /// 来源校验：只读比对文件哈希，报告 changed / missing / failed，不改任何状态（改状态用 sync）。
    pub async fn validate_sources(&self) -> Result<Vec<Value>> {
        let snapshots = self.open_snapshots().await?;
        let mut out = vec![];
        for snapshot in &snapshots {
            for record in snapshot.store.sources().await? {
                let state = match tokio::fs::read(&record.absolute_path).await {
                    Ok(bytes) if content_hash(&bytes) == record.content_hash => {
                        if record.status == SourceStatus::Failed {
                            "failed"
                        } else {
                            "current"
                        }
                    }
                    Ok(_) => "changed",
                    Err(_) => "missing",
                };
                if state != "current" {
                    out.push(json!({
                        "scope": snapshot.store.scope.as_str(), "documentId": record.document_id, "path": record.source_path,
                        "state": state, "entries": record.sections.values().collect::<Vec<_>>(),
                        "error": record.error.as_ref().map(|e| format!("{}: {}", e.stage, e.message)),
                    }));
                }
            }
        }
        Ok(out)
    }

    pub async fn audit_tail(&self, limit: usize) -> Result<Vec<Value>> {
        let snapshots = self.open_snapshots().await?;
        let mut events = vec![];
        for s in &snapshots {
            events.extend(s.store.read_audit(limit).await?);
        }
        events.sort_by(|a, b| a["at"].as_str().cmp(&b["at"].as_str()));
        let skip = events.len().saturating_sub(limit);
        Ok(events.into_iter().skip(skip).collect())
    }
}
