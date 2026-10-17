//! `git-checkpoint` 的 git 管道（TS `gitCheckpointRepo.ts`）。
//!
//! 核心不变量（`rust-u2-git.md` §8.2）：**不碰用户真实 index、不产生用户可见分支或普通 commit**。
//! checkpoint 用临时 `GIT_INDEX_FILE` 把 workspace 的 live 状态固化成隐藏 commit，
//! 再由 `refs/escode/checkpoints/<workspaceHash>/<checkpointId>` 挂住防止被 GC。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use super::GitService;
use super::Resolution;
use super::checkpoint_conflicts::{Conflict, affected_repo_paths, collect_workspace_conflicts};
use super::checkpoint_parse::{parse_name_status, parse_numstat};
use super::checkpoint_store::{GitCheckpointMeta, temp_index_root, workspace_hash};
use super::command::{GitRun, ensure_succeeded};
use super::paths::{normalize_git_path, to_workspace_relative_git_path};

/// TS `GitCheckpointFileDiff` 的 Rust 形状（`path` / `originalPath` 是**绝对路径**）。
pub struct FileDiff {
    pub path: String,
    pub repo_relative_path: String,
    pub workspace_relative_path: String,
    pub original_path: Option<String>,
    pub kind: &'static str,
    pub added: u64,
    pub removed: u64,
}

/// TS `GitCheckpointRestoreResult`：成功带 `restoredPaths`，被冲突挡住带 `conflicts`。
pub struct RestoreResult {
    pub success: bool,
    pub conflicts: Vec<Conflict>,
    pub restored_paths: Vec<String>,
}

/// checkpoint commit 的作者/提交者固定，不借用用户身份。
const CHECKPOINT_IDENTITY: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "ESCode Checkpoint"),
    ("GIT_AUTHOR_EMAIL", "checkpoint@escode.local"),
    ("GIT_COMMITTER_NAME", "ESCode Checkpoint"),
    ("GIT_COMMITTER_EMAIL", "checkpoint@escode.local"),
];

pub(crate) fn ref_name(workspace_path: &str, checkpoint_id: &str) -> String {
    format!(
        "refs/escode/checkpoints/{}/{}",
        workspace_hash(workspace_path),
        checkpoint_id
    )
}

fn to_absolute(repo_root: &str, repo_relative_path: &str) -> String {
    let mut path = PathBuf::from(repo_root);
    for segment in normalize_git_path(repo_relative_path).split('/') {
        if !segment.is_empty() {
            path.push(segment);
        }
    }
    path.to_string_lossy().into_owned()
}

/// 带临时 index 与 checkpoint 身份变量的 `git` 调用（`createCheckpoint` 的每一步都用它）。
fn with_temp_index<'a>(cwd: &'a str, args: Vec<String>, temp_index: &str) -> GitRun<'a> {
    let mut run = GitRun::args(cwd, args);
    run = run.env("GIT_INDEX_FILE", temp_index);
    for (key, value) in CHECKPOINT_IDENTITY {
        run = run.env(key, value);
    }
    run
}

/// TS `ensureRepository`：git 不可用 / 非仓库各自一条错误（文案逐字）。
pub(crate) async fn ensure_repository(
    service: &GitService,
    workspace_path: &str,
) -> Result<Resolution> {
    let resolution = service.resolve(workspace_path).await?;
    if !resolution.git_available {
        bail!("Git binary is not available in the current environment.");
    }
    if !resolution.is_repository {
        bail!("Workspace is not inside a Git repository.");
    }
    Ok(resolution)
}

/// TS `createCheckpoint`：临时 index → tree → 隐藏 commit → hidden ref。临时目录成功失败都清。
pub(crate) async fn create_checkpoint(
    service: &GitService,
    data_base_dir: &Path,
    workspace_path: &str,
    checkpoint_id: &str,
) -> Result<GitCheckpointMeta> {
    let resolution = ensure_repository(service, workspace_path).await?;
    let root = temp_index_root(data_base_dir);
    tokio::fs::create_dir_all(&root).await?;
    let temp_dir = root.join(format!("index-{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&temp_dir).await?;
    let temp_index = temp_dir.join("index");
    let temp_index_text = temp_index.to_string_lossy().into_owned();

    let result = create_with_temp_index(
        service,
        &resolution,
        workspace_path,
        checkpoint_id,
        &temp_index,
        &temp_index_text,
    )
    .await;
    let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    result
}

async fn create_with_temp_index(
    service: &GitService,
    resolution: &Resolution,
    workspace_path: &str,
    checkpoint_id: &str,
    temp_index: &Path,
    temp_index_text: &str,
) -> Result<GitCheckpointMeta> {
    let repo_root = resolution.repo_root.as_str();
    let name = ref_name(workspace_path, checkpoint_id);

    // 预热临时 index——纯性能路径，最终 tree 由 worktree 决定，语义等价：
    // ① 复制用户 index 以复用 stat cache（未改动文件走 stat-match，跳过读文件与写对象）；
    // ② 否则 `read-tree HEAD` 至少让已追踪文件走 hash-match；③ 再否则留空 index（全新仓库）。
    let index_path = service
        .git(GitRun::new(
            repo_root,
            &["rev-parse", "--git-path", "index"],
        ))
        .await?;
    if index_path.exit_code == Some(0) {
        let relative = index_path.stdout.trim();
        if !relative.is_empty() {
            let user_index = PathBuf::from(repo_root).join(normalize_git_path(relative));
            let _ = tokio::fs::copy(&user_index, temp_index).await;
        }
    }
    let _ = service
        .git(with_temp_index(
            repo_root,
            vec!["read-tree".to_owned(), "HEAD".to_owned()],
            temp_index_text,
        ))
        .await?;

    let add = service
        .git(with_temp_index(
            repo_root,
            vec![
                "add".to_owned(),
                "-A".to_owned(),
                "--".to_owned(),
                resolution.workspace_in_repo_path.clone(),
            ],
            temp_index_text,
        ))
        .await?;
    ensure_succeeded("git add checkpoint", &add, &[0])?;

    let tree = service
        .git(with_temp_index(
            repo_root,
            vec!["write-tree".to_owned()],
            temp_index_text,
        ))
        .await?;
    ensure_succeeded("git write-tree checkpoint", &tree, &[0])?;

    let commit = service
        .git(with_temp_index(
            repo_root,
            vec![
                "commit-tree".to_owned(),
                tree.stdout.trim().to_owned(),
                "-m".to_owned(),
                format!("escode checkpoint {checkpoint_id}"),
            ],
            temp_index_text,
        ))
        .await?;
    ensure_succeeded("git commit-tree checkpoint", &commit, &[0])?;
    let commit_oid = commit.stdout.trim().to_owned();

    let update = service
        .git(GitRun::args(
            repo_root,
            vec!["update-ref".to_owned(), name.clone(), commit_oid.clone()],
        ))
        .await?;
    ensure_succeeded("git update-ref checkpoint", &update, &[0])?;

    Ok(GitCheckpointMeta {
        checkpoint_id: checkpoint_id.to_owned(),
        workspace_path: workspace_path.to_owned(),
        repo_root: resolution.repo_root.clone(),
        workspace_in_repo_path: resolution.workspace_in_repo_path.clone(),
        created_at: serde_json::Number::from(now_ms()),
        ref_name: name,
        commit_oid,
        scope: "workspace".to_owned(),
    })
}

/// TS `computeCheckpointDiff`：`--name-status` 与 `--numstat` 两路并发后合并。
pub(crate) async fn compute_diff(
    service: &GitService,
    workspace_path: &str,
    from: &GitCheckpointMeta,
    to: &GitCheckpointMeta,
) -> Result<Vec<FileDiff>> {
    let resolution = ensure_repository(service, workspace_path).await?;
    let repo_root = resolution.repo_root.as_str();
    let diff_args = |mode: &str| {
        GitRun::args(
            repo_root,
            vec![
                "diff".to_owned(),
                mode.to_owned(),
                "--find-renames".to_owned(),
                "-z".to_owned(),
                from.commit_oid.clone(),
                to.commit_oid.clone(),
                "--".to_owned(),
                resolution.workspace_in_repo_path.clone(),
            ],
        )
    };
    let (name_status, numstat) = tokio::join!(
        service.git(diff_args("--name-status")),
        service.git(diff_args("--numstat")),
    );
    let name_status = name_status?;
    let numstat = numstat?;
    ensure_succeeded("git diff --name-status checkpoint", &name_status, &[0])?;
    ensure_succeeded("git diff --numstat checkpoint", &numstat, &[0])?;

    // TS `Map` 语义：同路径后写覆盖前写。
    let mut stats: HashMap<String, (u64, u64)> = HashMap::new();
    for (path, added, removed) in parse_numstat(&numstat.stdout) {
        stats.insert(path, (added, removed));
    }

    let mut files = Vec::new();
    for entry in parse_name_status(&name_status.stdout) {
        // TS `numstat.get(entry.path) ?? numstat.get(entry.originalPath ?? "") ?? {0, 0}`。
        let (added, removed) = stats
            .get(&entry.path)
            .or_else(|| {
                entry
                    .original_path
                    .as_ref()
                    .and_then(|original| stats.get(original))
            })
            .copied()
            .unwrap_or((0, 0));
        files.push(FileDiff {
            path: to_absolute(repo_root, &entry.path),
            workspace_relative_path: to_workspace_relative_git_path(
                &entry.path,
                &resolution.workspace_in_repo_path,
            ),
            repo_relative_path: entry.path.clone(),
            original_path: entry
                .original_path
                .as_ref()
                .map(|original| to_absolute(repo_root, original)),
            kind: entry.kind,
            added,
            removed,
        });
    }
    Ok(files)
}

/// TS `restoreBetweenCheckpoints`：三方语义（调用方声明的基线 / 目标 / 当前现实）。
pub(crate) async fn restore_between(
    service: &GitService,
    workspace_path: &str,
    from: &GitCheckpointMeta,
    to: &GitCheckpointMeta,
    force: bool,
) -> Result<RestoreResult> {
    let resolution = ensure_repository(service, workspace_path).await?;
    let files = compute_diff(service, workspace_path, from, to).await?;
    let affected = affected_repo_paths(&resolution.repo_root, &files);

    if affected.is_empty() {
        // 没有受影响路径：不查冲突、不报错（TS 同）。
        return Ok(RestoreResult {
            success: true,
            conflicts: Vec::new(),
            restored_paths: Vec::new(),
        });
    }

    let conflicts = collect_workspace_conflicts(service, &resolution, from, &affected).await?;
    if !conflicts.is_empty() && !force {
        // 默认模式：相关路径已偏离 fromCheckpoint 就拒绝写入，把冲突交给上层，不偷偷覆盖。
        return Ok(RestoreResult {
            success: false,
            conflicts,
            restored_paths: Vec::new(),
        });
    }

    let restore_paths: Vec<String> = files
        .iter()
        .filter(|file| file.kind != "deleted")
        .map(|file| file.repo_relative_path.clone())
        .collect();
    if !restore_paths.is_empty() {
        // 只改 worktree、不动真实 index——回滚是「文件状态恢复」，不是改用户暂存区。
        let mut args: Vec<String> = vec![
            "restore".to_owned(),
            format!("--source={}", to.commit_oid),
            "--worktree".to_owned(),
            "--".to_owned(),
        ];
        args.extend(restore_paths);
        let restore = service
            .git(GitRun::args(&resolution.repo_root, args))
            .await?;
        ensure_succeeded("git restore checkpoint", &restore, &[0])?;
    }

    // deleted 的路径要删残留；rename 的旧路径也要删（git restore 只写新路径，否则新旧并存）。
    for file in &files {
        if file.kind == "deleted" {
            remove_path(&file.path).await;
        } else if file.kind == "renamed"
            && let Some(original) = &file.original_path
        {
            remove_path(original).await;
        }
    }

    // 以 `to` 为基线复核：仍有偏差说明 restore 没把磁盘带到目标状态，抛错而不是固化成 success。
    let verify = collect_workspace_conflicts(service, &resolution, to, &affected).await?;
    if !verify.is_empty() {
        bail!("Checkpoint restore verification failed.");
    }

    Ok(RestoreResult {
        success: true,
        conflicts: Vec::new(),
        restored_paths: files.into_iter().map(|file| file.path).collect(),
    })
}

/// TS `removeFileIfExists`：`rm(path, {force: true, recursive: true})`（文件与目录都吃）。
async fn remove_path(path: &str) {
    if tokio::fs::remove_dir_all(path).await.is_ok() {
        return;
    }
    let _ = tokio::fs::remove_file(path).await;
}

/// TS `deleteCheckpoint`（repo 层）：删 hidden ref；ref 不存在（退出码 1）不算错误。
pub(crate) async fn delete_checkpoint_ref(
    service: &GitService,
    workspace_path: &str,
    checkpoint: &GitCheckpointMeta,
) -> Result<()> {
    let resolution = ensure_repository(service, workspace_path).await?;
    let result = service
        .git(GitRun::args(
            &resolution.repo_root,
            vec![
                "update-ref".to_owned(),
                "-d".to_owned(),
                checkpoint.ref_name.clone(),
            ],
        ))
        .await?;
    ensure_succeeded("git update-ref -d checkpoint", &result, &[0, 1])
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}
