//! `git.generateCommitMessage` 的 Git 侧输入（TS `gitService.generateCommitMessage` +
//! `commitMessageFileScope.ts`）：状态 → 候选文件 → 会话作用域过滤 → 差分查询。

use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::GitService;
use super::paths::{js_trim, normalize_git_path, normalize_workspace_in_repo_path};
use super::reads::changes_for_source;

/// TS `COMMIT_MESSAGE_DIFF_FILE_LIMIT`。
const DIFF_FILE_LIMIT: usize = 8;

pub struct CommitMessageInputs {
    pub branch_name: Option<String>,
    pub files: Vec<Value>,
    pub diffs: Vec<Value>,
}

/// TS `normalizeCommitMessageScopePath`：`normalizeGitPath(trim)` → 去开头 `./` 或 `/` → 去结尾 `/`。
fn scope_path(path: &str) -> String {
    let normalized = normalize_git_path(js_trim(path));
    let stripped = normalized
        .strip_prefix("./")
        .or_else(|| normalized.strip_prefix('/'))
        .unwrap_or(&normalized);
    stripped.trim_end_matches('/').to_owned()
}

fn add(scope: &mut BTreeSet<String>, path: &str) {
    let normalized = scope_path(path);
    if !normalized.is_empty() {
        scope.insert(normalized);
    }
}

fn inside(relative: &str) -> bool {
    let normalized = normalize_git_path(relative);
    !normalized.is_empty()
        && normalized != ".."
        && !normalized.starts_with("../")
        && !std::path::Path::new(&normalized).is_absolute()
}

/// POSIX `path.relative(from, to)`（两者都是绝对路径）。
fn relative(from: &str, to: &str) -> String {
    let split = |p: &str| {
        p.split('/')
            .filter(|s| !s.is_empty() && *s != ".")
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let (from, to) = (split(from), split(to));
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = vec!["..".to_owned(); from.len() - common];
    parts.extend(to[common..].iter().cloned());
    parts.join("/")
}

fn build_scope(
    workspace_path: &str,
    repo_root: &str,
    workspace_in_repo: &str,
    session_paths: &[String],
) -> Option<BTreeSet<String>> {
    let sources: Vec<&str> = session_paths
        .iter()
        .map(|p| js_trim(p))
        .filter(|p| !p.is_empty())
        .collect();
    if sources.is_empty() {
        return None;
    }
    let mut scope = BTreeSet::new();
    let in_repo = normalize_workspace_in_repo_path(workspace_in_repo);
    for path in sources {
        add(&mut scope, path);
        if std::path::Path::new(path).is_absolute() {
            let repo_relative = relative(repo_root, path);
            if inside(&repo_relative) {
                add(&mut scope, &repo_relative);
            }
            let workspace_relative = relative(workspace_path, path);
            if inside(&workspace_relative) {
                add(&mut scope, &workspace_relative);
            }
            continue;
        }
        let normalized = scope_path(path);
        if in_repo != "." && !normalized.starts_with(&format!("{in_repo}/")) {
            add(&mut scope, &format!("{in_repo}/{normalized}"));
        }
    }
    (!scope.is_empty()).then_some(scope)
}

fn in_scope(file: &Value, scope: &Option<BTreeSet<String>>) -> bool {
    let Some(scope) = scope else { return true };
    ["path", "repoRelativePath", "workspaceRelativePath"]
        .iter()
        .any(|key| scope.contains(&scope_path(file[*key].as_str().unwrap_or_default())))
}

impl GitService {
    pub async fn commit_message_inputs(
        &self,
        workspace_path: &str,
        include_unstaged: bool,
        session_paths: &[String],
    ) -> anyhow::Result<CommitMessageInputs> {
        let snapshot = self.status(workspace_path).await?;
        let mut files = if include_unstaged {
            changes_for_source(&snapshot, "unstaged")
        } else {
            Vec::new()
        };
        files.extend(changes_for_source(&snapshot, "staged"));
        let scope = build_scope(
            workspace_path,
            &snapshot.resolution.repo_root,
            &snapshot.resolution.workspace_in_repo_path,
            session_paths,
        );
        files.retain(|file| in_scope(file, &scope));
        if files.is_empty() {
            anyhow::bail!("There are no changes available to commit.");
        }
        let mut queries: Vec<(String, &str)> = Vec::new();
        for file in files.iter().take(DIFF_FILE_LIMIT) {
            let path = file["path"].as_str().unwrap_or_default().to_owned();
            if file["section"] == "staged" {
                queries.push((path, "staged"));
            } else if include_unstaged {
                queries.push((path, "unstaged"));
            }
        }
        let results =
            futures_util::future::join_all(queries.iter().map(|(path, source)| async move {
                let query =
                    json!({"workspacePath": workspace_path, "path": path, "sourceId": source});
                self.diff(&query).await
            }))
            .await;
        // `Promise.allSettled`：失败的查询丢弃；只留有 patch 或 summary 的。
        let diffs = results
            .into_iter()
            .filter_map(Result::ok)
            .filter(|diff| {
                diff["patch"].as_str().is_some_and(|s| !s.is_empty())
                    || diff["summary"].as_str().is_some_and(|s| !s.is_empty())
            })
            .collect();
        Ok(CommitMessageInputs {
            branch_name: snapshot.summary["branchName"].as_str().map(str::to_owned),
            files,
            diffs,
        })
    }
}

#[cfg(test)]
#[path = "commit_message_tests.rs"]
mod tests;
