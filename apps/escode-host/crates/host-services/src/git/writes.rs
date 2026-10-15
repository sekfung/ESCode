//! git 域的写方法（TS `gitCliRepo.ts` 的 switch / create / stage / unstage / discard / commit / push）。

use serde_json::{Value, json};

use super::command::{GitRun, ensure_succeeded};
use super::parse;
use super::paths::{self, js_trim};
use super::{GitService, Resolution, Snapshot, ensure_repository, files, normalize_input_path};

/// TS `GIT_OPERATION_MARKERS`。
const OPERATION_MARKERS: [&str; 7] = [
    "MERGE_HEAD",
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
    "REBASE_HEAD",
    "rebase-merge",
    "rebase-apply",
    "BISECT_LOG",
];

fn mutation(
    action: &str,
    branch: Option<&str>,
    ok: bool,
    did_change: bool,
    created: bool,
    summary: &Value,
    issues: Vec<Value>,
) -> Value {
    json!({
        "ok": ok,
        "action": action,
        "branchName": branch,
        "didChange": did_change,
        "created": created,
        "summary": summary,
        "issues": issues,
    })
}

fn failure(action: &str, branch: Option<&str>, summary: &Value, issues: Vec<Value>) -> Value {
    mutation(action, branch, false, false, false, summary, issues)
}

fn simple_issue(code: &str, message: &str) -> Value {
    json!({"code": code, "message": message})
}

/// 去重保序（TS `Array.from(new Set(...))`）。
pub(super) fn dedupe(paths: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    paths
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

impl GitService {
    pub(super) async fn normalize_all(
        &self,
        resolution: &Resolution,
        inputs: &[String],
    ) -> anyhow::Result<Vec<String>> {
        let mut out = Vec::with_capacity(inputs.len());
        for path in inputs {
            out.push(normalize_input_path(resolution, path).await?);
        }
        Ok(dedupe(out))
    }

    /// TS `hasOperationInProgress`：git-dir 标记文件探测。
    async fn operation_in_progress(&self, resolution: &Resolution) -> anyhow::Result<bool> {
        let mut args = vec!["rev-parse".to_owned()];
        for marker in OPERATION_MARKERS {
            args.push("--git-path".to_owned());
            args.push(marker.to_owned());
        }
        let output = self.git(GitRun::args(&resolution.repo_root, args)).await?;
        ensure_succeeded("git rev-parse --git-path", &output, &[0])?;
        for line in output.stdout.replace("\r\n", "\n").split('\n') {
            let line = js_trim(line);
            if line.is_empty() {
                continue;
            }
            let candidate = if paths::is_absolute(line) {
                line.to_owned()
            } else {
                paths::resolve(&resolution.repo_root, &[line])
            };
            if files::exists(&candidate).await {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// TS `validateBranchName`。
    async fn branch_name_issue(
        &self,
        resolution: &Resolution,
        branch: &str,
    ) -> anyhow::Result<Option<Value>> {
        let output = self
            .git(GitRun::new(
                &resolution.repo_root,
                &["check-ref-format", "--branch", branch],
            ))
            .await?;
        if output.timed_out || output.output_truncated {
            ensure_succeeded("git check-ref-format --branch", &output, &[0])?;
        }
        Ok((output.exit_code != Some(0))
            .then(|| parse::invalid_branch_name_issue(Some(&output.stderr))))
    }

    /// switch 与 create 共用的阻塞判定：冲突 → 进行中操作 → 分支名非法。
    async fn mutation_blocker(
        &self,
        snapshot: &Snapshot,
        action: &str,
        branch: &str,
    ) -> anyhow::Result<Option<Value>> {
        if snapshot.entries.iter().any(|entry| entry.is_conflicted) {
            return Ok(Some(failure(
                action,
                Some(branch),
                &snapshot.summary,
                vec![simple_issue(
                    "conflicts-present",
                    "Repository still has unresolved conflicts.",
                )],
            )));
        }
        if self.operation_in_progress(&snapshot.resolution).await? {
            return Ok(Some(failure(
                action,
                Some(branch),
                &snapshot.summary,
                vec![simple_issue(
                    "operation-in-progress",
                    "Another Git operation is still in progress.",
                )],
            )));
        }
        if let Some(issue) = self.branch_name_issue(&snapshot.resolution, branch).await? {
            return Ok(Some(failure(
                action,
                Some(branch),
                &snapshot.summary,
                vec![issue],
            )));
        }
        Ok(None)
    }

    /// TS `switchBranch`。
    pub async fn switch_branch(&self, workspace_path: &str, target: &str) -> anyhow::Result<Value> {
        let snapshot = self.status(workspace_path).await?;
        ensure_repository(&snapshot.resolution, "switch branches")?;
        let branch = js_trim(target);
        if branch.is_empty() {
            return Ok(failure(
                "switch",
                None,
                &snapshot.summary,
                vec![parse::invalid_branch_name_issue(None)],
            ));
        }
        if snapshot.summary["headRefType"] == "branch" && snapshot.summary["branchName"] == branch {
            return Ok(mutation(
                "switch",
                Some(branch),
                true,
                false,
                false,
                &snapshot.summary,
                vec![],
            ));
        }
        if let Some(blocked) = self.mutation_blocker(&snapshot, "switch", branch).await? {
            return Ok(blocked);
        }
        let output = self
            .git(GitRun::new(
                &snapshot.resolution.repo_root,
                &["switch", "--no-guess", branch],
            ))
            .await?;
        if output.exit_code != Some(0) {
            return Ok(failure(
                "switch",
                Some(branch),
                &snapshot.summary,
                parse::branch_mutation_issues(&output),
            ));
        }
        let next = self.status(workspace_path).await?;
        Ok(mutation(
            "switch",
            Some(branch),
            true,
            true,
            false,
            &next.summary,
            vec![],
        ))
    }

    /// TS `createBranchAndSwitch`（`startPoint` 前插 `--` 防选项注入）。
    pub async fn create_branch_and_switch(
        &self,
        workspace_path: &str,
        branch_name: &str,
        start_point: Option<&str>,
    ) -> anyhow::Result<Value> {
        let snapshot = self.status(workspace_path).await?;
        ensure_repository(&snapshot.resolution, "create and switch branches")?;
        let branch = js_trim(branch_name);
        if branch.is_empty() {
            return Ok(failure(
                "create-and-switch",
                None,
                &snapshot.summary,
                vec![parse::invalid_branch_name_issue(None)],
            ));
        }
        if let Some(blocked) = self
            .mutation_blocker(&snapshot, "create-and-switch", branch)
            .await?
        {
            return Ok(blocked);
        }
        let start = start_point.map(js_trim).filter(|start| !start.is_empty());
        let mut args = vec!["switch", "--no-guess", "-c", branch];
        if let Some(start) = start {
            args.extend(["--", start]);
        }
        let output = self
            .git(GitRun::new(&snapshot.resolution.repo_root, &args))
            .await?;
        if output.exit_code != Some(0) {
            return Ok(failure(
                "create-and-switch",
                Some(branch),
                &snapshot.summary,
                parse::branch_mutation_issues(&output),
            ));
        }
        let next = self.status(workspace_path).await?;
        Ok(mutation(
            "create-and-switch",
            Some(branch),
            true,
            true,
            true,
            &next.summary,
            vec![],
        ))
    }

    async fn path_command(
        &self,
        workspace_path: &str,
        inputs: &[String],
        label: &str,
        head: &[&str],
        failure_label: &str,
    ) -> anyhow::Result<()> {
        let resolution = self.resolve(workspace_path).await?;
        ensure_repository(&resolution, label)?;
        let paths = self.normalize_all(&resolution, inputs).await?;
        if paths.is_empty() {
            return Ok(());
        }
        let mut args: Vec<String> = head.iter().map(|arg| (*arg).to_owned()).collect();
        args.push("--".to_owned());
        args.extend(paths);
        let output = self.git(GitRun::args(&resolution.repo_root, args)).await?;
        ensure_succeeded(failure_label, &output, &[0])
    }

    pub async fn stage(&self, workspace_path: &str, inputs: &[String]) -> anyhow::Result<()> {
        self.path_command(workspace_path, inputs, "stage paths", &["add"], "git add")
            .await
    }

    pub async fn unstage(&self, workspace_path: &str, inputs: &[String]) -> anyhow::Result<()> {
        self.path_command(
            workspace_path,
            inputs,
            "unstage paths",
            &["restore", "--staged"],
            "git restore --staged",
        )
        .await
    }

    pub async fn discard(
        &self,
        workspace_path: &str,
        inputs: &[String],
        staged: bool,
    ) -> anyhow::Result<()> {
        let head: &[&str] = if staged {
            &["restore", "--source=HEAD", "--staged", "--worktree"]
        } else {
            &["restore", "--worktree"]
        };
        self.path_command(workspace_path, inputs, "discard paths", head, "git restore")
            .await
    }
}
