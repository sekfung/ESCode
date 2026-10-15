//! git 域的 diff 读取（TS `gitCliRepo.ts` 的 `getDiff` / `readGitBlobPreviewContent` / `withDiffContents`）。

use serde_json::Value;

use super::command::{self, GitRun};
use super::paths::{self, js_trim};
use super::{GitService, files, normalize_input_path};

impl GitService {
    /// TS `getDiff`。
    pub async fn diff(&self, query: &Value) -> anyhow::Result<Value> {
        let workspace_path = query["workspacePath"].as_str().unwrap_or_default();
        let path = query["path"].as_str().unwrap_or_default();
        let source = query["sourceId"].as_str().unwrap_or_default();
        let resolution = self.resolve(workspace_path).await?;
        let absolute = paths::absolute_from_workspace(workspace_path, path);
        if !resolution.git_available {
            return Ok(files::unavailable_diff(
                &absolute,
                "Git binary is not available in the current environment.",
            ));
        }
        if !resolution.is_repository {
            return Ok(files::unavailable_diff(
                &absolute,
                "Workspace is not inside a Git repository.",
            ));
        }
        let relative = normalize_input_path(&resolution, path).await?;
        let root = resolution.repo_root.as_str();
        if source == "branch" {
            let snapshot = self.status(workspace_path).await?;
            let Some(tracking) = snapshot.summary["trackingBranchName"].as_str() else {
                return Ok(files::unavailable_diff(
                    &absolute,
                    "Current branch does not have an upstream branch.",
                ));
            };
            let output = self
                .git(diff_run(
                    root,
                    &[
                        "diff",
                        "--no-ext-diff",
                        "--no-color",
                        "--binary",
                        "--find-renames",
                    ],
                    Some(&format!("{tracking}...HEAD")),
                    &relative,
                ))
                .await?;
            let parsed = files::to_diff_result(
                &absolute,
                &output,
                &[0],
                "No branch comparison diff is available for this file.",
            );
            let merge_base = self
                .git(GitRun::new(root, &["merge-base", tracking, "HEAD"]))
                .await?;
            let base = if merge_base.exit_code == Some(0) {
                js_trim(&merge_base.stdout).to_owned()
            } else {
                String::new()
            };
            let before = self
                .blob_preview(
                    root,
                    if base.is_empty() { tracking } else { &base },
                    &relative,
                )
                .await?;
            let after = self.blob_preview(root, "HEAD", &relative).await?;
            return Ok(with_contents(parsed, before.zip(after)));
        }
        let staged = query["staged"].as_bool().unwrap_or(source == "staged");
        let mut args = vec!["diff"];
        if staged {
            args.push("--cached");
        }
        args.extend(["--no-ext-diff", "--no-color", "--binary"]);
        let output = self.git(diff_run(root, &args, None, &relative)).await?;
        let parsed = files::to_diff_result(
            &absolute,
            &output,
            &[0],
            "No Git diff is available for this file.",
        );
        if parsed["availability"] != "unavailable" || staged {
            let contents = if staged {
                let before = self.blob_preview(root, "HEAD", &relative).await?;
                let after = self.blob_preview(root, "", &relative).await?;
                before.zip(after)
            } else {
                let before = self.blob_preview(root, "", &relative).await?;
                let after = files::working_tree_preview(&absolute).await;
                before.zip(after)
            };
            return Ok(with_contents(parsed, contents));
        }
        // 未跟踪文件：先给稳定的单文件 unified patch；不可文本预览再退回 `--no-index`。
        if !files::exists(&absolute).await {
            return Ok(parsed);
        }
        if let Some(untracked) = files::untracked_text_diff(&absolute, &relative).await {
            return Ok(untracked);
        }
        let null_device = if cfg!(windows) { "NUL" } else { "/dev/null" };
        let output = self
            .git(
                GitRun::new(
                    root,
                    &[
                        "diff",
                        "--no-index",
                        "--no-ext-diff",
                        "--no-color",
                        "--binary",
                        null_device,
                        &absolute,
                    ],
                )
                .timeout(command::DIFF_TIMEOUT)
                .max_output(command::DIFF_BYTES),
            )
            .await?;
        Ok(files::to_diff_result(
            &absolute,
            &output,
            &[0, 1],
            "No previewable diff is available for this file.",
        ))
    }

    /// TS `readGitBlobPreviewContent`：`git show <ref>:<path>`，失败/超时/截断/含 NUL → None。
    async fn blob_preview(
        &self,
        root: &str,
        reference: &str,
        relative: &str,
    ) -> anyhow::Result<Option<String>> {
        let output = self
            .git(
                GitRun::args(root, vec!["show".into(), format!("{reference}:{relative}")])
                    .timeout(command::DIFF_TIMEOUT)
                    .max_output(command::DIFF_BYTES),
            )
            .await?;
        Ok((!output.timed_out
            && !output.output_truncated
            && output.exit_code == Some(0)
            && !output.stdout.contains('\0'))
        .then_some(output.stdout))
    }
}

fn diff_run<'a>(root: &'a str, head: &[&str], range: Option<&str>, relative: &str) -> GitRun<'a> {
    let mut args: Vec<String> = head.iter().map(|arg| (*arg).to_owned()).collect();
    if let Some(range) = range {
        args.push(range.to_owned());
    }
    args.push("--".to_owned());
    args.push(relative.to_owned());
    GitRun::args(root, args)
        .timeout(command::DIFF_TIMEOUT)
        .max_output(command::DIFF_BYTES)
}

/// TS `withDiffContents`：只有 patch 且两侧全文都读到时才补全文（任一侧失败整体降级）。
fn with_contents(mut diff: Value, contents: Option<(String, String)>) -> Value {
    if diff["availability"] != "patch" {
        return diff;
    }
    if let Some((before, after)) = contents {
        diff["beforeContent"] = Value::from(before);
        diff["afterContent"] = Value::from(after);
    }
    diff
}
