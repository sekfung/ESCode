//! `git-checkpoint` 的冲突检测（TS `gitCheckpointRepo.ts` 的 `collectWorkspaceConflicts`）。
//!
//! 判定口径不是「整个工作区是否 dirty」，而是**restore 将触达的这些路径**当前是否仍等于调用方声明的
//! `fromCheckpoint`。只有这样才能在存在无关改动时依然安全恢复，而不是把整个仓库一刀切判成不可恢复。
//!
//! 比对用 **blob hash**（`git hash-object --no-filters` 对 tree 条目的 objectId），
//! 不比时间戳或大小——弱信号会把「内容相同但 mtime 变了」误判成冲突。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::GitService;
use super::Resolution;
use super::checkpoint::FileDiff;
use super::checkpoint_parse::{TreeEntry, parse_ls_tree};
use super::checkpoint_store::GitCheckpointMeta;
use super::command::{GitRun, ensure_succeeded};
use super::paths::{normalize_git_path, to_workspace_relative_git_path};

/// TS `GitCheckpointConflict`。
pub struct Conflict {
    /// 绝对路径。
    pub path: String,
    pub repo_relative_path: String,
    pub workspace_relative_path: String,
    /// `content-mismatch` / `missing-in-worktree` / `unexpected-file-in-worktree` / `type-mismatch`。
    pub reason: &'static str,
}

/// TS `toAbsolutePath(repoRoot, path)`：按 `/` 拆段后拼接（git 路径恒为正斜杠）。
fn to_absolute_path(repo_root: &str, repo_relative_path: &str) -> PathBuf {
    let mut path = PathBuf::from(repo_root);
    for segment in normalize_git_path(repo_relative_path).split('/') {
        if !segment.is_empty() {
            path.push(segment);
        }
    }
    path
}

/// TS `normalizeAffectedRepoPath(repoRoot, path)`：绝对路径折算成 repo 相对，其余原样。
pub(crate) fn normalize_affected_repo_path(repo_root: &str, path: &str) -> String {
    let normalized = normalize_git_path(path);
    if !Path::new(&normalized).is_absolute() {
        return normalized;
    }
    let prefix = format!("{}{}", repo_root, std::path::MAIN_SEPARATOR);
    normalize_git_path(normalized.strip_prefix(&prefix).unwrap_or(&normalized))
}

/// TS `buildAffectedRepoPaths` + 紧随其后的 `.map(normalizeAffectedRepoPath)`：
/// 新路径与 rename 的旧路径合并去重（旧路径在 diff 里是绝对路径，归一化后与 repo 相对路径可比）。
pub(crate) fn affected_repo_paths(repo_root: &str, files: &[FileDiff]) -> Vec<String> {
    let mut values: Vec<String> = Vec::new();
    for file in files {
        for candidate in [Some(&file.repo_relative_path), file.original_path.as_ref()] {
            let Some(candidate) = candidate else {
                continue;
            };
            let normalized = normalize_affected_repo_path(repo_root, candidate);
            if !values.contains(&normalized) {
                values.push(normalized);
            }
        }
    }
    values
}

pub(crate) async fn collect_workspace_conflicts(
    service: &GitService,
    resolution: &Resolution,
    from: &GitCheckpointMeta,
    affected: &[String],
) -> Result<Vec<Conflict>> {
    if affected.is_empty() {
        return Ok(Vec::new());
    }

    // 一次批量 ls-tree（TS 同）：拿到 fromCheckpoint 期望的文件类型与 blob hash。
    let mut args: Vec<String> = vec![
        "ls-tree".to_owned(),
        "-r".to_owned(),
        "-z".to_owned(),
        from.commit_oid.clone(),
        "--".to_owned(),
    ];
    args.extend(affected.iter().cloned());
    let tree_result = service
        .git(GitRun::args(&resolution.repo_root, args))
        .await?;
    ensure_succeeded("git ls-tree checkpoint paths", &tree_result, &[0])?;
    let tree: BTreeMap<String, TreeEntry> =
        parse_ls_tree(&tree_result.stdout).into_iter().collect();

    let build = |repo_relative_path: &str, reason: &'static str| Conflict {
        path: to_absolute_path(&resolution.repo_root, repo_relative_path)
            .to_string_lossy()
            .into_owned(),
        repo_relative_path: repo_relative_path.to_owned(),
        workspace_relative_path: to_workspace_relative_git_path(
            repo_relative_path,
            &resolution.workspace_in_repo_path,
        ),
        reason,
    };

    let mut conflicts: Vec<Conflict> = Vec::new();
    for repo_relative_path in affected {
        let absolute = to_absolute_path(&resolution.repo_root, repo_relative_path);
        let Some(expected) = tree.get(repo_relative_path) else {
            // fromCheckpoint 里没有这个路径：它本来就不该在磁盘上。现在存在 = 之后被人新增，覆盖有风险。
            if tokio::fs::symlink_metadata(&absolute).await.is_err() {
                continue;
            }
            conflicts.push(build(repo_relative_path, "unexpected-file-in-worktree"));
            continue;
        };

        let Ok(stats) = tokio::fs::symlink_metadata(&absolute).await else {
            // fromCheckpoint 要求它存在、磁盘上却没有：直接写回会盖掉「文件为何消失」的真实操作。
            conflicts.push(build(repo_relative_path, "missing-in-worktree"));
            continue;
        };

        let expects_symlink = expected.mode == "120000";
        if stats.is_dir() || (expects_symlink && !stats.is_symlink()) {
            // 期待文件/符号链接、磁盘却是目录或另一种类型：直接 restore 会语义错位。
            conflicts.push(build(repo_relative_path, "type-mismatch"));
            continue;
        }

        let hash_result = service
            .git(GitRun::args(
                &resolution.repo_root,
                vec![
                    "hash-object".to_owned(),
                    "--no-filters".to_owned(),
                    absolute.to_string_lossy().into_owned(),
                ],
            ))
            .await?;
        ensure_succeeded("git hash-object checkpoint verify", &hash_result, &[0])?;
        if hash_result.stdout.trim() == expected.object_id {
            continue;
        }
        conflicts.push(build(repo_relative_path, "content-mismatch"));
    }

    // TS 用 `Map` 去重：同一 repoRelativePath 只留第一条（保持出现序）。
    let mut deduped: Vec<Conflict> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for item in conflicts {
        if seen.contains(&item.repo_relative_path) {
            continue;
        }
        seen.push(item.repo_relative_path.clone());
        deduped.push(item);
    }
    Ok(deduped)
}
