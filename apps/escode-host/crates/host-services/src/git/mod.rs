//! `git` 域（docs/specs/rust-u2-git.md；TS 真相源 `packages/services/src/git/`）。
//!
//! Git CLI 是唯一事实源：每次调用重新执行 `git`。唯一的进程态是 `collapsed`（某 repoRoot 的详细
//! status 一旦超出输出上限，此后一律目录折叠），它改变结果，必须照搬。

mod command;
mod commit_message;
pub use commit_message::CommitMessageInputs;
pub mod commit_message_prompt;
// `git-checkpoint`（rust-u2-git.md §8）：store / git 管道 / 冲突检测 / 服务层。
pub mod checkpoint;
pub mod checkpoint_conflicts;
pub mod checkpoint_parse;
pub mod checkpoint_service;
pub mod checkpoint_store;
mod files;
mod parse;
mod parse_refs;
pub mod paths;
mod reads;
mod reads_diff;
mod writes;
mod writes_commit;

use escode_host_control::logging;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use command::{GitOutput, GitRun, ensure_succeeded};
use parse::{NumstatMap, StatusEntry};
use paths::{js_trim, normalize_workspace_in_repo_path};

/// 排序比较器（TS `localeCompare`；由 host-rpc 注入 ICU 实现）。
pub type LocaleCompare = Arc<dyn Fn(&str, &str) -> Ordering + Send + Sync>;

/// Host 内唯一的 git 服务。
pub struct GitService {
    env: BTreeMap<String, String>,
    candidates: Vec<String>,
    binary: tokio::sync::OnceCell<Option<String>>,
    collapsed: Mutex<BTreeSet<String>>,
    locale_compare: LocaleCompare,
    temp_root: PathBuf,
}

/// TS `GitResolvedRepository`。
#[derive(Clone, Debug)]
pub(crate) struct Resolution {
    pub workspace_path: String,
    pub repo_root: String,
    pub workspace_in_repo_path: String,
    pub watch_paths: Vec<(String, bool)>,
    pub git_available: bool,
    pub is_repository: bool,
}

/// TS `GitStatusSnapshot`。
pub(crate) struct Snapshot {
    pub resolution: Resolution,
    pub summary: Value,
    pub entries: Vec<StatusEntry>,
    pub staged: NumstatMap,
    pub unstaged: NumstatMap,
    pub untracked: NumstatMap,
}

impl GitService {
    /// `process_env`：Host 进程 env（组合根经 `core_api::env` 唯一出口取得）；`temp_root`：临时目录根。
    pub fn new(
        process_env: &[(String, String)],
        temp_root: PathBuf,
        locale_compare: LocaleCompare,
    ) -> Self {
        Self {
            env: command::command_env(process_env),
            candidates: command::binary_candidates(process_env),
            binary: tokio::sync::OnceCell::new(),
            collapsed: Mutex::default(),
            locale_compare,
            temp_root,
        }
    }

    /// TS `resolveGitBinary`：首个 `--version` 成功的候选，进程内缓存。
    async fn binary(&self) -> Option<String> {
        self.binary
            .get_or_init(|| async {
                for candidate in &self.candidates {
                    if command::can_execute(candidate, &self.env).await {
                        return Some(candidate.clone());
                    }
                }
                None
            })
            .await
            .clone()
    }

    /// TS `commandProvider.run`：二进制不可用时抛错（`Git binary is not available`）。
    pub(crate) async fn git(&self, run: GitRun<'_>) -> anyhow::Result<GitOutput> {
        let binary = self
            .binary()
            .await
            .ok_or_else(|| anyhow::anyhow!("Git binary is not available"))?;
        Ok(command::run(&binary, &self.env, run).await)
    }

    /// TS `resolveRepository`。
    pub(crate) async fn resolve(&self, workspace_path: &str) -> anyhow::Result<Resolution> {
        let unavailable = |git_available: bool| Resolution {
            workspace_path: workspace_path.to_owned(),
            repo_root: workspace_path.to_owned(),
            workspace_in_repo_path: ".".to_owned(),
            watch_paths: Vec::new(),
            git_available,
            is_repository: false,
        };
        if self.binary().await.is_none() {
            return Ok(unavailable(false));
        }
        let output = self
            .git(GitRun::new(
                workspace_path,
                &[
                    "rev-parse",
                    "--show-toplevel",
                    "--show-prefix",
                    "--absolute-git-dir",
                    "--git-common-dir",
                ],
            ))
            .await?;
        if output.exit_code != Some(0) {
            // 目录被并发删除 / 非仓库：按「当前非可用仓库」降级（TS 同）。
            if is_missing_working_directory(&output) || is_not_repository(&output) {
                return Ok(unavailable(true));
            }
            ensure_succeeded("git rev-parse", &output, &[0])?;
        }
        let normalized = output.stdout.replace("\r\n", "\n");
        let lines: Vec<&str> = normalized.split('\n').collect();
        let repo_root = lines.first().map(|line| js_trim(line)).unwrap_or_default();
        if repo_root.is_empty() {
            anyhow::bail!("Failed to resolve Git repository root");
        }
        Ok(Resolution {
            workspace_path: workspace_path.to_owned(),
            repo_root: repo_root.to_owned(),
            workspace_in_repo_path: normalize_workspace_in_repo_path(
                lines.get(1).copied().unwrap_or_default(),
            ),
            watch_paths: watch_paths(
                workspace_path,
                lines.get(2).map(|line| js_trim(line)).unwrap_or_default(),
                lines.get(3).map(|line| js_trim(line)).unwrap_or_default(),
            ),
            git_available: true,
            is_repository: true,
        })
    }

    async fn run_status(&self, resolution: &Resolution) -> anyhow::Result<GitOutput> {
        let collapsed = lock(&self.collapsed).contains(&resolution.repo_root);
        let status = |mode: &str| {
            GitRun::args(
                &resolution.repo_root,
                vec![
                    "status".into(),
                    "--porcelain=v2".into(),
                    "--branch".into(),
                    format!("--untracked-files={mode}"),
                    "-z".into(),
                ],
            )
        };
        let output = self
            .git(status(if collapsed { "normal" } else { "all" }))
            .await?;
        if collapsed || !output.output_truncated {
            return Ok(output);
        }
        // 大仓库逐文件未跟踪状态超限：按 repoRoot 记住目录折叠模式（不重复必然失败的详细命令）。
        lock(&self.collapsed).insert(resolution.repo_root.clone());
        logging::warn(
            "git",
            format!(
                "status detailed output exceeded limit; collapsing untracked directories repoRoot={}",
                resolution.repo_root
            ),
        );
        self.git(status("normal")).await
    }

    /// TS `getStatus`。
    pub(crate) async fn status(&self, workspace_path: &str) -> anyhow::Result<Snapshot> {
        let resolution = self.resolve(workspace_path).await?;
        if !resolution.git_available || !resolution.is_repository {
            return Ok(Snapshot {
                summary: empty_summary(&resolution),
                resolution,
                entries: Vec::new(),
                staged: NumstatMap::default(),
                unstaged: NumstatMap::default(),
                untracked: NumstatMap::default(),
            });
        }
        let numstat = |cached: bool| {
            let mut args = vec!["diff"];
            if cached {
                args.push("--cached");
            }
            args.extend(["--numstat", "-z", "--find-renames", "--"]);
            GitRun::new(&resolution.repo_root, &args)
        };
        let (status, staged, unstaged) = tokio::join!(
            self.run_status(&resolution),
            self.git(numstat(true)),
            self.git(numstat(false)),
        );
        let (status, staged, unstaged) = (status?, staged?, unstaged?);
        ensure_succeeded("git status", &status, &[0])?;
        ensure_succeeded("git diff --cached --numstat", &staged, &[0])?;
        ensure_succeeded("git diff --numstat", &unstaged, &[0])?;
        let parsed = parse::parse_status_porcelain(&status.stdout);
        let untracked = files::untracked_stats(&resolution.repo_root, &parsed.entries).await;
        let summary = json!({
            "workspacePath": resolution.workspace_path,
            "repoRoot": resolution.repo_root,
            "workspaceInRepoPath": resolution.workspace_in_repo_path,
            "autoRefreshWatchPaths": watch_paths_json(&resolution.watch_paths),
            "branchName": parsed.branch_name,
            "trackingBranchName": parsed.tracking_branch_name,
            "headRefType": parsed.head_ref_type,
            "ahead": parsed.ahead,
            "behind": parsed.behind,
            "isDirty": !parsed.entries.is_empty(),
            "isGitAvailable": true,
            "isRepository": true,
        });
        Ok(Snapshot {
            resolution,
            summary,
            entries: parsed.entries,
            staged: parse::parse_numstat(&staged.stdout),
            unstaged: parse::parse_numstat(&unstaged.stdout),
            untracked,
        })
    }
}

/// TS `createEmptySummary`。
fn empty_summary(resolution: &Resolution) -> Value {
    json!({
        "workspacePath": resolution.workspace_path,
        "repoRoot": resolution.repo_root,
        "workspaceInRepoPath": resolution.workspace_in_repo_path,
        "autoRefreshWatchPaths": watch_paths_json(&resolution.watch_paths),
        "branchName": null,
        "trackingBranchName": null,
        "headRefType": "branch",
        "ahead": 0,
        "behind": 0,
        "isDirty": false,
        "isGitAvailable": resolution.git_available,
        "isRepository": resolution.is_repository,
    })
}

fn watch_paths_json(paths: &[(String, bool)]) -> Value {
    Value::Array(
        paths
            .iter()
            .map(|(path, recursive)| json!({"path": path, "recursive": recursive}))
            .collect(),
    )
}

/// TS `normalizeWatchPath`：根路径（`/`、`C:`、`C:\`）原样，其余去尾分隔符。
fn normalize_watch_path(path: &str) -> String {
    let trimmed = js_trim(path);
    let bytes = trimmed.as_bytes();
    let drive_root = (bytes.len() == 2 || bytes.len() == 3)
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes.len() == 2 || matches!(bytes[2], b'/' | b'\\'));
    if trimmed == "/" || drive_root {
        return trimmed.to_owned();
    }
    trimmed.trim_end_matches(['/', '\\']).to_owned()
}

/// TS `buildAutoRefreshWatchPaths`：git-dir 与 common-dir（相对结果以 workspace 为基准解析）。
fn watch_paths(
    workspace_path: &str,
    absolute_git_dir: &str,
    git_common_dir: &str,
) -> Vec<(String, bool)> {
    let mut paths: Vec<(String, bool)> = Vec::new();
    let mut add = |path: &str| {
        let normalized = normalize_watch_path(path);
        if !normalized.is_empty() && !paths.iter().any(|(existing, _)| *existing == normalized) {
            paths.push((normalized, true));
        }
    };
    add(absolute_git_dir);
    let common = if git_common_dir.is_empty() {
        absolute_git_dir.to_owned()
    } else if paths::is_absolute(git_common_dir) {
        git_common_dir.to_owned()
    } else {
        paths::resolve(workspace_path, &[git_common_dir])
    };
    add(&common);
    paths
}

/// TS `isNotRepositoryResult`。
fn is_not_repository(output: &GitOutput) -> bool {
    let stderr = output.stderr.to_lowercase();
    stderr.contains("not a git repository") || stderr.contains("outside repository")
}

/// TS `isMissingWorkingDirectoryResult`。
fn is_missing_working_directory(output: &GitOutput) -> bool {
    let stderr = output.stderr.to_lowercase();
    (output.exit_code == Some(-2) && stderr.contains("enoent"))
        || stderr.contains("unable to read current working directory")
        || stderr.contains("no such file or directory")
}

/// TS `ensureRepositoryAvailable`。
fn ensure_repository(resolution: &Resolution, label: &str) -> anyhow::Result<()> {
    if !resolution.git_available {
        anyhow::bail!("Cannot {label}: Git binary is not available");
    }
    if !resolution.is_repository {
        anyhow::bail!("Cannot {label}: workspace is not inside a Git repository");
    }
    Ok(())
}

/// TS `normalizeInputPath`：realpath（失败回退原路径）后相对 repoRoot；越界抛错。
async fn normalize_input_path(resolution: &Resolution, path: &str) -> anyhow::Result<String> {
    let raw = paths::absolute_from_workspace(&resolution.workspace_path, path);
    let absolute = match tokio::fs::canonicalize(&raw).await {
        Ok(real) => strip_verbatim(&real.to_string_lossy()),
        Err(_) => raw,
    };
    let relative = paths::normalize_git_path(&paths::relative(&resolution.repo_root, &absolute));
    if relative.is_empty() || relative == "." || relative == ".." || relative.starts_with("../") {
        anyhow::bail!("Path is outside repository scope: {path}");
    }
    Ok(relative)
}

/// Windows `canonicalize` 带 `\\?\` 前缀，Node `realpath` 不带。
fn strip_verbatim(path: &str) -> String {
    path.strip_prefix(r"\\?\").unwrap_or(path).to_owned()
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
