//! Review 工作流的变更文件（spec §9.1）：`git diff --name-only HEAD` + 未跟踪文件。
//! 参数数组调用、5 秒超时、任何失败返回空（检索照常进行，只是少了变更上下文）。
use std::time::Duration;

const GIT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_FILES: usize = 200;

async fn git(root: &str, args: &[&str]) -> Vec<String> {
    let child = tokio::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(GIT_TIMEOUT, child).await {
        Ok(Ok(output)) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect(),
        _ => vec![],
    }
}

pub async fn changed_files(root: &str) -> Vec<String> {
    let mut files = git(root, &["diff", "--name-only", "HEAD"]).await;
    files.extend(git(root, &["ls-files", "--others", "--exclude-standard"]).await);
    files.sort();
    files.dedup();
    files.truncate(MAX_FILES);
    files
}
