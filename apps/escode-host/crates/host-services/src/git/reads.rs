//! git 域的读方法（TS `gitService.ts` 的投影 + `gitCliRepo.ts` 的读路径）。

use serde_json::{Value, json};

use super::command::{GitRun, ensure_succeeded};
use super::parse::{self, LineStat, StatusEntry};
use super::paths::{self, is_path_in_workspace_scope, to_workspace_relative_git_path};
use super::{GitService, Snapshot, files, normalize_input_path};

/// TS `DEFAULT_GIT_GRAPH_MAX_COUNT` / `MAX_GIT_GRAPH_MAX_COUNT`。
const GRAPH_DEFAULT: i64 = 100;
const GRAPH_MAX: i64 = 200;

fn in_scope(workspace_in_repo: &str, path: &str, original: Option<&str>) -> bool {
    is_path_in_workspace_scope(path, workspace_in_repo)
        || original.is_some_and(|original| is_path_in_workspace_scope(original, workspace_in_repo))
}

/// TS `buildFileChange`（`x`/`y` 为 null 时整键省略）。
fn file_change(
    snapshot: &Snapshot,
    entry: &StatusEntry,
    section: &str,
    added: i64,
    removed: i64,
) -> Value {
    let mut change = json!({
        "path": paths::to_absolute_path(&snapshot.resolution.repo_root, &entry.path),
        "repoRelativePath": entry.path,
        "workspaceRelativePath": to_workspace_relative_git_path(&entry.path, &snapshot.resolution.workspace_in_repo_path),
    });
    if let Some(x) = &entry.x {
        change["x"] = Value::from(x.clone());
    }
    if let Some(y) = &entry.y {
        change["y"] = Value::from(y.clone());
    }
    for (key, value) in [
        ("kind", json!(entry.kind)),
        ("section", json!(section)),
        ("added", json!(added)),
        ("removed", json!(removed)),
        ("isStaged", json!(section == "staged")),
        ("isUntracked", json!(section == "untracked")),
        ("isConflicted", json!(section == "conflicted")),
    ] {
        change[key] = value;
    }
    change
}

fn stat_or_zero(stat: Option<&LineStat>) -> (i64, i64) {
    stat.map_or((0, 0), |stat| (stat.added, stat.removed))
}

/// TS `toChangeForSource`。
fn change_for_source(snapshot: &Snapshot, entry: &StatusEntry, source: &str) -> Option<Value> {
    if !in_scope(
        snapshot.summary["workspaceInRepoPath"]
            .as_str()
            .unwrap_or("."),
        &entry.path,
        entry.original_path.as_deref(),
    ) {
        return None;
    }
    if source == "staged" {
        let x = entry.x.as_deref();
        if entry.is_untracked || entry.is_conflicted || x.is_none_or(|x| x.is_empty() || x == ".") {
            return None;
        }
        let (added, removed) = stat_or_zero(snapshot.staged.get(&entry.path));
        return Some(file_change(snapshot, entry, "staged", added, removed));
    }
    if entry.is_conflicted {
        return Some(file_change(snapshot, entry, "conflicted", 0, 0));
    }
    if entry.is_untracked {
        let (added, removed) = stat_or_zero(snapshot.untracked.get(&entry.path));
        // 无可审查增删的未跟踪条目不进列表；目录折叠记录（末尾 `/`）必须保留为降级入口。
        if !(entry.path.ends_with('/') || added > 0 || removed > 0) {
            return None;
        }
        return Some(file_change(snapshot, entry, "untracked", added, removed));
    }
    if entry.y.as_deref().is_none_or(|y| y.is_empty() || y == ".") {
        return None;
    }
    let (added, removed) = stat_or_zero(snapshot.unstaged.get(&entry.path));
    Some(file_change(snapshot, entry, "unstaged", added, removed))
}

/// TS `getChangesForSource`。
pub(crate) fn changes_for_source(snapshot: &Snapshot, source: &str) -> Vec<Value> {
    snapshot
        .entries
        .iter()
        .filter_map(|entry| change_for_source(snapshot, entry, source))
        .collect()
}

impl GitService {
    pub async fn repository_summary(&self, workspace_path: &str) -> anyhow::Result<Value> {
        Ok(self.status(workspace_path).await?.summary)
    }

    pub async fn changes(&self, workspace_path: &str, source: &str) -> anyhow::Result<Value> {
        let snapshot = self.status(workspace_path).await?;
        Ok(Value::Array(changes_for_source(&snapshot, source)))
    }

    /// TS `getWorkspaceRepositoryInfo`。
    pub async fn workspace_repository_info(&self, workspace_path: &str) -> anyhow::Result<Value> {
        let resolution = self.resolve(workspace_path).await?;
        if !resolution.git_available || !resolution.is_repository {
            return Ok(json!({
                "workspacePath": workspace_path,
                "kind": "not-repository",
                "isGitAvailable": resolution.git_available,
            }));
        }
        let kind = files::linked_worktree_kind(&resolution.repo_root).await;
        Ok(json!({"workspacePath": workspace_path, "kind": kind, "isGitAvailable": true}))
    }

    /// TS `listLocalBranches`。
    pub async fn local_branches(&self, workspace_path: &str) -> anyhow::Result<Value> {
        let snapshot = self.status(workspace_path).await?;
        let summary = &snapshot.summary;
        if !snapshot.resolution.git_available || !snapshot.resolution.is_repository {
            return Ok(json!({
                "headRefType": summary["headRefType"],
                "currentBranchName": summary["branchName"],
                "branches": [],
            }));
        }
        let output = self
            .git(GitRun::new(
                &snapshot.resolution.repo_root,
                &[
                    "for-each-ref",
                    "refs/heads",
                    "--format=%(refname:short)%00%(upstream:short)%00%(objectname)%00%(committerdate:unix)",
                ],
            ))
            .await?;
        ensure_succeeded("git for-each-ref refs/heads", &output, &[0])?;
        let current = (summary["headRefType"] == "branch")
            .then(|| summary["branchName"].as_str())
            .flatten();
        let compare = std::sync::Arc::clone(&self.locale_compare);
        Ok(json!({
            "headRefType": summary["headRefType"],
            "currentBranchName": summary["branchName"],
            "branches": parse::parse_branch_refs(&output.stdout, current, |a, b| compare(a, b)),
        }))
    }

    /// TS `getCommitGraph`。
    pub async fn commit_graph(
        &self,
        workspace_path: &str,
        max_count: Option<f64>,
        skip: Option<f64>,
    ) -> anyhow::Result<Value> {
        let empty = json!({"commits": [], "hasMore": false});
        let resolution = self.resolve(workspace_path).await?;
        if !resolution.git_available || !resolution.is_repository {
            return Ok(empty);
        }
        let max_count = max_count
            .filter(|value| value.is_finite())
            .map_or(GRAPH_DEFAULT, |value| {
                (value.floor() as i64).clamp(1, GRAPH_MAX)
            });
        let skip = skip
            .filter(|value| value.is_finite())
            .map_or(0, |value| (value.floor() as i64).max(0));
        let output = self
            .git(GitRun::args(
                &resolution.repo_root,
                vec![
                    "log".into(),
                    "HEAD".into(),
                    "--branches".into(),
                    "--tags".into(),
                    "--remotes".into(),
                    "--date-order".into(),
                    "--topo-order".into(),
                    format!("--skip={skip}"),
                    format!("--max-count={}", max_count + 1),
                    "--format=%H%x00%P%x00%an%x00%at%x00%s%x00%D%x1e".into(),
                ],
            ))
            .await?;
        if output.exit_code != Some(0) {
            let stderr = output.stderr.to_lowercase();
            if [
                "does not have any commits yet",
                "your current branch",
                "bad default revision",
                "ambiguous argument 'head'",
            ]
            .iter()
            .any(|marker| stderr.contains(marker))
            {
                return Ok(empty);
            }
            ensure_succeeded("git log visible refs", &output, &[0])?;
        }
        let commits = parse::parse_graph_records(&output.stdout);
        let limit = usize::try_from(max_count).unwrap_or(usize::MAX);
        let has_more = commits.len() > limit;
        Ok(
            json!({"commits": commits.into_iter().take(limit).collect::<Vec<_>>(), "hasMore": has_more}),
        )
    }

    /// TS `getIgnoredPaths`。
    pub async fn ignored_paths(
        &self,
        workspace_path: &str,
        inputs: &[String],
    ) -> anyhow::Result<Value> {
        if inputs.is_empty() {
            return Ok(json!([]));
        }
        let resolution = self.resolve(workspace_path).await?;
        if !resolution.git_available || !resolution.is_repository {
            return Ok(json!([]));
        }
        let mut pairs: Vec<(String, String)> = Vec::new();
        for path in inputs {
            if let Ok(relative) = normalize_input_path(&resolution, path).await {
                pairs.push((
                    paths::absolute_from_workspace(&resolution.workspace_path, path),
                    relative,
                ));
            }
        }
        if pairs.is_empty() {
            return Ok(json!([]));
        }
        let mut args = vec!["check-ignore".to_owned(), "--".to_owned()];
        args.extend(pairs.iter().map(|(_, relative)| relative.clone()));
        let output = self.git(GitRun::args(&resolution.repo_root, args)).await?;
        if output.exit_code == Some(1) {
            return Ok(json!([]));
        }
        ensure_succeeded("git check-ignore", &output, &[0])?;
        let ignored: std::collections::BTreeSet<String> = output
            .stdout
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line))
            .filter(|line| !line.is_empty())
            .map(|line| line.replace('\\', "/"))
            .collect();
        Ok(Value::Array(
            pairs
                .into_iter()
                .filter(|(_, relative)| ignored.contains(relative))
                .map(|(absolute, _)| Value::from(absolute))
                .collect(),
        ))
    }

    /// TS `getBranchComparison` + `toBranchComparisonChange`。
    pub async fn branch_comparison(&self, workspace_path: &str) -> anyhow::Result<Value> {
        let snapshot = self.status(workspace_path).await?;
        self.comparison_of(&snapshot).await
    }

    pub(crate) async fn comparison_of(&self, snapshot: &Snapshot) -> anyhow::Result<Value> {
        let summary = &snapshot.summary;
        let tracking = summary["trackingBranchName"].as_str();
        let head = summary["branchName"].as_str().unwrap_or("HEAD");
        let (Some(tracking), true, true) = (
            tracking,
            snapshot.resolution.git_available,
            snapshot.resolution.is_repository,
        ) else {
            return Ok(json!({
                "baseRef": summary["trackingBranchName"],
                "headRef": head,
                "comparisonLabel": null,
                "changes": [],
            }));
        };
        let output = self
            .git(GitRun::args(
                &snapshot.resolution.repo_root,
                vec![
                    "diff".into(),
                    "--numstat".into(),
                    "-z".into(),
                    "--find-renames".into(),
                    format!("{tracking}...HEAD"),
                    "--".into(),
                ],
            ))
            .await?;
        ensure_succeeded("git diff --numstat upstream...HEAD", &output, &[0])?;
        let resolution = &snapshot.resolution;
        let changes: Vec<Value> = parse::parse_numstat(&output.stdout)
            .0
            .into_iter()
            .filter(|(path, stat)| {
                in_scope(&resolution.workspace_in_repo_path, path, stat.original_path.as_deref())
            })
            .map(|(path, stat)| {
                json!({
                    "path": paths::to_absolute_path(&resolution.repo_root, &path),
                    "repoRelativePath": path,
                    "workspaceRelativePath": to_workspace_relative_git_path(&path, &resolution.workspace_in_repo_path),
                    "kind": parse::kind_from_numstat(&stat),
                    "section": "branch",
                    "added": stat.added,
                    "removed": stat.removed,
                    "isStaged": false,
                    "isUntracked": false,
                    "isConflicted": false,
                })
            })
            .collect();
        let label = match summary["branchName"].as_str() {
            Some(branch) => format!("{branch} -> {tracking}"),
            None => format!("HEAD -> {tracking}"),
        };
        Ok(json!({
            "baseRef": tracking,
            "headRef": head,
            "comparisonLabel": label,
            "changes": changes,
        }))
    }

    /// TS `getIdentity`。
    pub async fn identity(&self, workspace_path: &str) -> anyhow::Result<Value> {
        let resolution = self.resolve(workspace_path).await?;
        if !resolution.git_available || !resolution.is_repository {
            return Ok(json!({
                "userName": null, "userEmail": null, "nameSource": null, "emailSource": null, "scopeLabel": null,
            }));
        }
        let query = |key: &str| {
            GitRun::args(
                &resolution.repo_root,
                ["config", "--show-scope", "--show-origin", "--get", key]
                    .iter()
                    .map(|arg| (*arg).to_owned())
                    .collect(),
            )
        };
        let (name, email) =
            tokio::join!(self.git(query("user.name")), self.git(query("user.email")));
        let (name_scope, name_source, name_value) = parse::parse_config_value(&name?)?;
        let (email_scope, email_source, email_value) = parse::parse_config_value(&email?)?;
        Ok(json!({
            "userName": name_value,
            "userEmail": email_value,
            "nameSource": name_source,
            "emailSource": email_source,
            "scopeLabel": name_scope.or(email_scope),
        }))
    }

    /// TS `refresh`：一次 status 快照切出 summary / unstaged / staged（+ 可选 identity 与分支对比）。
    pub async fn refresh(
        &self,
        workspace_path: &str,
        include_identity: bool,
        include_branch_comparison: bool,
    ) -> anyhow::Result<Value> {
        let identity = async {
            if include_identity {
                self.identity(workspace_path).await.map(Some)
            } else {
                Ok(None)
            }
        };
        let comparison = async {
            if include_branch_comparison {
                self.branch_comparison(workspace_path).await.map(Some)
            } else {
                Ok(None)
            }
        };
        let (snapshot, identity, comparison) =
            tokio::join!(self.status(workspace_path), identity, comparison);
        let snapshot = snapshot?;
        Ok(json!({
            "summary": snapshot.summary,
            "identity": identity?,
            "unstagedChanges": changes_for_source(&snapshot, "unstaged"),
            "stagedChanges": changes_for_source(&snapshot, "staged"),
            "branchComparison": comparison?,
        }))
    }
}
