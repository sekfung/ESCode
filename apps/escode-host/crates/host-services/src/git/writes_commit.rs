//! git 域的提交与推送（TS `gitCliRepo.ts` 的 `commit` / `push` / `resolvePushRemote`）。

use serde_json::{Value, json};

use super::command::{self, GitRun, ensure_succeeded};
use super::parse;
use super::paths::js_trim;
use super::writes::dedupe;
use super::{GitService, Resolution, Snapshot, ensure_repository};

impl GitService {
    /// TS `commit`（+ `gitService.commit` 回读 summary）。
    pub async fn commit(
        &self,
        workspace_path: &str,
        message: &str,
        inputs: Option<&[String]>,
        staged_only: bool,
    ) -> anyhow::Result<Value> {
        let resolution = self.resolve(workspace_path).await?;
        ensure_repository(&resolution, "commit changes")?;
        let message = js_trim(message);
        if message.is_empty() {
            anyhow::bail!("Commit message cannot be empty");
        }
        let paths = match inputs {
            Some(inputs) if !inputs.is_empty() => self.normalize_all(&resolution, inputs).await?,
            _ => Vec::new(),
        };
        let commit_hash = if staged_only && !paths.is_empty() {
            self.commit_selected_staged(&resolution, message, &paths)
                .await?
        } else {
            let mut args = vec!["commit".to_owned(), "-m".to_owned(), message.to_owned()];
            if !paths.is_empty() {
                args.push("--".to_owned());
                args.extend(paths);
            }
            let output = self.git(GitRun::args(&resolution.repo_root, args)).await?;
            ensure_succeeded("git commit", &output, &[0])?;
            let hash = self
                .git(GitRun::new(&resolution.repo_root, &["rev-parse", "HEAD"]))
                .await?;
            ensure_succeeded("git rev-parse HEAD", &hash, &[0])?;
            js_trim(&hash.stdout).to_owned()
        };
        let status = self.status(workspace_path).await?;
        Ok(json!({"commitHash": commit_hash, "summary": status.summary}))
    }

    /// TS stagedOnly 分支：临时 index 只提交选中路径的已暂存版本，其余已暂存文件留在真实 index。
    async fn commit_selected_staged<'a>(
        &self,
        resolution: &'a Resolution,
        message: &str,
        paths: &[String],
    ) -> anyhow::Result<String> {
        let root = resolution.repo_root.as_str();
        let mut status_args = vec![
            "status".to_owned(),
            "--porcelain=v2".to_owned(),
            "-z".to_owned(),
            "--".to_owned(),
        ];
        status_args.extend(paths.iter().cloned());
        let scoped = self.git(GitRun::args(root, status_args)).await?;
        ensure_succeeded("git status selected paths", &scoped, &[0])?;
        let mut cleanup: Vec<String> = paths.to_vec();
        cleanup.extend(
            parse::parse_status_porcelain(&scoped.stdout)
                .entries
                .into_iter()
                .filter(|entry| paths.contains(&entry.path))
                .filter_map(|entry| entry.original_path)
                .filter(|path| !path.is_empty()),
        );
        let cleanup = dedupe(cleanup);
        let mut ls_args = vec![
            "ls-files".to_owned(),
            "--stage".to_owned(),
            "-z".to_owned(),
            "--".to_owned(),
        ];
        ls_args.extend(paths.iter().cloned());
        let staged = self.git(GitRun::args(root, ls_args)).await?;
        ensure_succeeded("git ls-files selected staged entries", &staged, &[0])?;
        let entries = parse::parse_index_entries(&staged.stdout)?;
        if entries.iter().any(|entry| entry.stage != "0") {
            anyhow::bail!("Cannot commit selected staged paths while index conflicts exist.");
        }
        let head = self
            .git(GitRun::new(root, &["rev-parse", "--verify", "HEAD"]))
            .await?;
        let parent = (head.exit_code == Some(0)).then(|| js_trim(&head.stdout).to_owned());
        let temp_dir = self.temp_root.join(format!(
            "escode-git-index-{}",
            uuid::Uuid::new_v4().simple()
        ));
        tokio::fs::create_dir_all(&temp_dir).await?;
        let index_path = temp_dir.join("index").to_string_lossy().into_owned();
        let result = async {
            let with_index = |run: GitRun<'a>| run.env("GIT_INDEX_FILE", &index_path);
            let read_tree = match &parent {
                Some(parent) => GitRun::new(root, &["read-tree", parent]),
                None => GitRun::new(root, &["read-tree", "--empty"]),
            };
            let output = self.git(with_index(read_tree)).await?;
            ensure_succeeded("git read-tree selected commit base", &output, &[0])?;
            if !cleanup.is_empty() {
                let mut args = vec![
                    "update-index".to_owned(),
                    "--force-remove".to_owned(),
                    "--".to_owned(),
                ];
                args.extend(cleanup.iter().cloned());
                let output = self.git(with_index(GitRun::args(root, args))).await?;
                ensure_succeeded("git update-index remove selected paths", &output, &[0])?;
            }
            for entry in &entries {
                let output = self
                    .git(with_index(GitRun::new(
                        root,
                        &[
                            "update-index",
                            "--add",
                            "--cacheinfo",
                            &entry.mode,
                            &entry.object_hash,
                            &entry.path,
                        ],
                    )))
                    .await?;
                ensure_succeeded("git update-index add selected paths", &output, &[0])?;
            }
            let output = self
                .git(with_index(GitRun::new(root, &["commit", "-m", message])))
                .await?;
            ensure_succeeded("git commit selected staged paths", &output, &[0])?;
            let hash = self.git(GitRun::new(root, &["rev-parse", "HEAD"])).await?;
            ensure_succeeded("git rev-parse selected commit HEAD", &hash, &[0])?;
            let commit_hash = js_trim(&hash.stdout).to_owned();
            // 只把已提交路径同步到新 HEAD，保留其它已暂存文件。
            let mut args = vec![
                "reset".to_owned(),
                "--quiet".to_owned(),
                "HEAD".to_owned(),
                "--".to_owned(),
            ];
            args.extend(cleanup.iter().cloned());
            let output = self.git(GitRun::args(root, args)).await?;
            ensure_succeeded("git reset selected committed paths", &output, &[0])?;
            Ok::<_, anyhow::Error>(commit_hash)
        }
        .await;
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
        result
    }

    /// TS `readOptionalGitConfig`。
    async fn optional_config(
        &self,
        resolution: &Resolution,
        key: &str,
    ) -> anyhow::Result<Option<String>> {
        let output = self
            .git(GitRun::new(
                &resolution.repo_root,
                &["config", "--get", key],
            ))
            .await?;
        let label = format!("git config --get {key}");
        if output.timed_out || output.output_truncated {
            ensure_succeeded(&label, &output, &[0])?;
        }
        match output.exit_code {
            Some(0) => {
                let value = js_trim(&output.stdout);
                Ok((!value.is_empty()).then(|| value.to_owned()))
            }
            Some(1) => Ok(None),
            _ => {
                ensure_succeeded(&label, &output, &[0])?;
                Ok(None)
            }
        }
    }

    /// TS `resolvePushRemote`。
    async fn push_remote(&self, snapshot: &Snapshot) -> anyhow::Result<String> {
        let resolution = &snapshot.resolution;
        ensure_repository(resolution, "push changes")?;
        let branch = snapshot.summary["branchName"]
            .as_str()
            .map(js_trim)
            .unwrap_or_default();
        if branch.is_empty() {
            anyhow::bail!("Cannot resolve a Git push remote without a current branch.");
        }
        if let Some(remote) = self
            .optional_config(resolution, &format!("branch.{branch}.remote"))
            .await?
        {
            return Ok(remote);
        }
        if let Some(remote) = self
            .optional_config(resolution, "remote.pushDefault")
            .await?
        {
            return Ok(remote);
        }
        let output = self
            .git(GitRun::new(&resolution.repo_root, &["remote"]))
            .await?;
        ensure_succeeded("git remote", &output, &[0])?;
        let remotes = parse::parse_remote_list(&output.stdout);
        if remotes.iter().any(|remote| remote == "origin") {
            return Ok("origin".to_owned());
        }
        match remotes.len() {
            1 => Ok(remotes[0].clone()),
            0 => anyhow::bail!("No Git remote is configured for the current repository."),
            _ => anyhow::bail!(
                "Multiple Git remotes are configured. Configure branch.<name>.remote or remote.pushDefault first."
            ),
        }
    }

    /// TS `push`。
    pub async fn push(&self, workspace_path: &str) -> anyhow::Result<Value> {
        let snapshot = self.status(workspace_path).await?;
        ensure_repository(&snapshot.resolution, "push changes")?;
        let branch = snapshot.summary["branchName"]
            .as_str()
            .map(js_trim)
            .unwrap_or_default()
            .to_owned();
        if snapshot.summary["headRefType"] != "branch" || branch.is_empty() {
            anyhow::bail!("Cannot push while HEAD is detached.");
        }
        let tracking = snapshot.summary["trackingBranchName"]
            .as_str()
            .filter(|value| !value.is_empty());
        let remote = match tracking {
            Some(tracking) => parse::tracking_remote_name(Some(tracking)),
            None => Some(self.push_remote(&snapshot).await?),
        };
        let args: Vec<String> = if tracking.is_some() {
            vec!["push".into()]
        } else {
            vec![
                "push".into(),
                "--set-upstream".into(),
                remote.clone().unwrap_or_else(|| "origin".into()),
                branch.clone(),
            ]
        };
        // push 是显式用户动作，可能被 pre-push hook 拉长：单独放宽超时与输出上限。
        let output = self
            .git(
                GitRun::args(&snapshot.resolution.repo_root, args)
                    .timeout(command::PUSH_TIMEOUT)
                    .max_output(command::PUSH_OUTPUT_BYTES),
            )
            .await?;
        ensure_succeeded("git push", &output, &[0])?;
        let next = self.status(workspace_path).await?;
        let next_tracking = next.summary["trackingBranchName"].as_str();
        Ok(json!({
            "branchName": branch,
            "trackingBranchName": next.summary["trackingBranchName"],
            "remoteName": remote.or_else(|| parse::tracking_remote_name(next_tracking)),
            "setUpstream": tracking.is_none(),
            "summary": next.summary,
        }))
    }
}
