//! 仓库类插件源（docs/specs/rust-plugin-marketplace-write.md W3）：`github` / `git` / `url`（git）/ `git-subdir`。
//! 对齐 TS `resolveRepositoryPluginSource`：公开 GitHub HTTPS 仓库先走 Archive（zipball，无需本机 Git），
//! 需要完整 Git 语义（非 GitHub、401/403/404、符号链接、submodule、LFS）时回退到系统 `git clone`。

use super::plugin_zip::{self, ZipDownloadError, ZipRoot};
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};
use std::time::Duration;

const CLONE_MAX_ATTEMPTS: u32 = 3;
const COMMAND_TIMEOUT: Duration = Duration::from_millis(90_000);
const RETRY_DELAY_MS: u64 = 1_000;

/// TS `PluginSourceMaterializationError`：带诊断 code 的源物化错误（安装诊断直接取 code）。
#[derive(Debug)]
pub(super) struct SourceError {
    pub code: &'static str,
    message: String,
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SourceError {}

/// TS `GitHubArchiveRequiresGitError`。
#[derive(Debug)]
struct RequiresGit(String);

impl std::fmt::Display for RequiresGit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GitHub Archive requires system Git fallback: {}", self.0)
    }
}

impl std::error::Error for RequiresGit {}

/// 解析出的源根；`cleanup` 删除临时目录（Archive 解压目录或 clone 目录）。
pub(super) struct RepoRoot {
    pub path: PathBuf,
    temp: PathBuf,
}

impl RepoRoot {
    pub(super) fn temp_dir(&self) -> PathBuf {
        self.temp.clone()
    }
}

pub(super) struct RepoSource {
    pub url: String,
    pub path: Option<String>,
    pub r#ref: Option<String>,
    pub sha: Option<String>,
}

/// TS `redactPluginSource`：URL 去掉 userinfo；`user:pass@host` 形式整体替换。
fn redact_source(source: &str) -> String {
    let trimmed = source.trim();
    if let Ok(mut url) = url::Url::parse(trimmed) {
        let _ = url.set_username("");
        let _ = url.set_password(None);
        return url.to_string();
    }
    let looks_like_credentials = trimmed
        .split_once('@')
        .is_some_and(|(user, _)| !user.contains(char::is_whitespace) && user.contains(':'));
    if looks_like_credentials {
        "configured Git source".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// TS `redactPluginDiagnosticText`：文本里的 URL 去凭据，`a:b@c` 片段整体替换。
fn redact_text(text: &str) -> String {
    text.split(' ')
        .map(|word| {
            if word.contains("://") {
                redact_source(word)
            } else if let Some((user, _)) = word.split_once('@')
                && user.contains(':')
                && !user.starts_with(':')
            {
                "configured Git source".to_owned()
            } else {
                word.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// TS `parsePublicGitHubRepositoryUrl`。
fn public_github(value: &str) -> Option<(String, String)> {
    let url = url::Url::parse(value).ok()?;
    let host = url.host_str()?.to_lowercase();
    if url.scheme() != "https"
        || !(host == "github.com" || host == "www.github.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let segments: Vec<&str> = url.path().split('/').filter(|s| !s.is_empty()).collect();
    let [owner, repo] = segments.as_slice() else {
        return None;
    };
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    let valid = |s: &str| {
        !s.is_empty()
            && s != "."
            && s != ".."
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
    };
    (valid(owner) && valid(repo)).then(|| ((*owner).to_owned(), repo.to_owned()))
}

fn resolve_inside(base: &Path, relative: &str) -> Option<PathBuf> {
    let joined = super::lexical_path::normalize(&base.join(relative));
    joined
        .starts_with(super::lexical_path::normalize(base))
        .then_some(joined)
}

/// TS `directoryDeclaresGitLfs`。
fn declares_lfs(dir: &Path, recursive: bool) -> bool {
    if let Ok(text) = std::fs::read_to_string(dir.join(".gitattributes"))
        && text
            .lines()
            .any(|line| line.split_whitespace().any(|token| token == "filter=lfs"))
    {
        return true;
    }
    recursive
        && std::fs::read_dir(dir).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|e| e.file_type().is_ok_and(|t| t.is_dir()) && declares_lfs(&e.path(), true))
        })
}

/// TS `detectRequiredGitSemantics`：Archive 不物化 submodule 与 LFS 对象，这类仓库必须走完整 Git。
fn requires_git(repository: &Path, selected: &Path) -> Option<&'static str> {
    if repository.join(".gitmodules").is_file() {
        return Some("repository declares Git submodules");
    }
    if declares_lfs(repository, selected == repository) {
        return Some("repository declares Git LFS filters");
    }
    if selected != repository {
        let mut current = selected.parent();
        while let Some(dir) = current {
            if dir == repository {
                break;
            }
            if declares_lfs(dir, false) {
                return Some("selected plugin path inherits Git LFS filters");
            }
            current = dir.parent();
        }
        if declares_lfs(selected, true) {
            return Some("selected plugin path declares Git LFS filters");
        }
    }
    None
}

/// TS `resolveGitHubArchiveSource`。
async fn github_archive(source: &RepoSource) -> Result<RepoRoot> {
    let (owner, repo) = public_github(&source.url).ok_or_else(|| {
        anyhow::Error::new(RequiresGit(format!(
            "source is not a public GitHub HTTPS repository: {}",
            source.url
        )))
    })?;
    let pin = source
        .sha
        .clone()
        .or_else(|| source.r#ref.clone())
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "HEAD".to_owned());
    let pin: String = url::form_urlencoded::byte_serialize(pin.as_bytes()).collect();
    let root: ZipRoot = plugin_zip::resolve_http(plugin_zip::Source {
        url: format!("https://api.github.com/repos/{owner}/{repo}/zipball/{pin}"),
        sha256: None,
        path: None,
        strip_root: Some(true),
        headers: vec![
            ("Accept".into(), "application/vnd.github+json".into()),
            ("User-Agent".into(), "ZCode-Plugin-Installer".into()),
        ],
        require_single_root: true,
    })
    .await?;
    let selected = match &source.path {
        Some(path) => match resolve_inside(&root.path, path).filter(|p| p.is_dir()) {
            Some(dir) => dir,
            None => {
                root.cleanup();
                bail!("Plugin source subdirectory does not exist: {path}");
            }
        },
        None => root.path.clone(),
    };
    if let Some(reason) = requires_git(&root.path, &selected) {
        root.cleanup();
        return Err(RequiresGit(reason.to_owned()).into());
    }
    Ok(RepoRoot {
        path: selected,
        temp: root.temp_dir(),
    })
}

/// TS `shouldFallbackGitHubArchiveToGit`。
fn should_fallback(error: &anyhow::Error) -> bool {
    if error.downcast_ref::<RequiresGit>().is_some() {
        return true;
    }
    if error
        .downcast_ref::<ZipDownloadError>()
        .is_some_and(|e| matches!(e.status, 401 | 403 | 404))
    {
        return true;
    }
    let message = error.to_string().to_lowercase();
    message.contains("plugin zip entry symlinks are not supported")
        || message.contains("unsupported plugin zip entry type")
}

/// TS `execGitCommand`：`ZCODE_GIT_BINARY` 可覆盖 git 路径；子进程环境按 host 规则清洗并恢复出网配置。
async fn git(args: &[String]) -> Result<()> {
    let binary = std::env::var("ZCODE_GIT_BINARY")
        .ok()
        .map(|b| b.trim().to_owned())
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| "git".to_owned());
    let mut command = tokio::process::Command::new(&binary);
    command.args(args).kill_on_drop(true);
    command.stdin(std::process::Stdio::null());
    zcode_cli_host::child_env::apply(&mut command, true);
    let output = match tokio::time::timeout(COMMAND_TIMEOUT, command.output()).await {
        Err(_) => bail!("Command timed out: {binary} {}", args.join(" ")),
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            let source = args
                .iter()
                .find(|a| a.contains("://") || a.starts_with("git@") || a.starts_with("git+"))
                .or(args.last())
                .map(String::as_str)
                .unwrap_or("Git operation");
            return Err(SourceError {
                code: "plugin_git_unavailable",
                message: format!(
                    "System Git is required for plugin source {}, but git is unavailable on this Agent Host. Install Git on the Agent Host, or use a public GitHub HTTPS or verified ZIP source.",
                    redact_source(source)
                ),
            }
            .into());
        }
        Ok(Err(error)) => return Err(error.into()),
        Ok(Ok(output)) => output,
    };
    if !output.status.success() {
        bail!(
            "Command failed: {binary} {}\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// TS `isRetryableGitCloneError`。
fn retryable(error: &anyhow::Error) -> bool {
    let text = error.to_string().to_lowercase();
    [
        "rpc failed",
        "operation timed out",
        "recv failure",
        "expected flush",
        "early eof",
        "remote end hung up",
        "http/2 stream",
        "connection reset",
        "etimedout",
        "econnreset",
        "network timeout",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

/// TS `clonePluginSource` + `resolveGitPluginSource`。
async fn git_clone(source: &RepoSource) -> Result<RepoRoot> {
    let dir = std::env::temp_dir().join(format!("zcode-plugin-src-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir)?;
    let mut args = vec!["clone".to_owned()];
    if source.sha.is_none() {
        args.extend(["--depth".to_owned(), "1".to_owned()]);
    }
    if let Some(r#ref) = &source.r#ref {
        args.extend(["--branch".to_owned(), r#ref.clone()]);
    }
    args.push(source.url.clone());
    args.push(dir.to_string_lossy().into_owned());
    let cloned = async {
        let mut attempt = 1;
        loop {
            if attempt > 1 {
                let _ = std::fs::remove_dir_all(&dir);
                std::fs::create_dir_all(&dir)?;
            }
            match git(&args).await {
                Ok(()) => break,
                Err(error) if attempt < CLONE_MAX_ATTEMPTS && retryable(&error) => {
                    // 只对网络型错误短重试，权限 / 仓库不存在等确定性错误直接失败。
                    tokio::time::sleep(Duration::from_millis(RETRY_DELAY_MS * u64::from(attempt)))
                        .await;
                    attempt += 1;
                }
                Err(error) => return Err(error),
            }
        }
        if let Some(sha) = &source.sha {
            git(&[
                "-C".to_owned(),
                dir.to_string_lossy().into_owned(),
                "checkout".to_owned(),
                sha.clone(),
            ])
            .await?;
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;
    if let Err(error) = cloned {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(error);
    }
    let path = match &source.path {
        None => dir.clone(),
        Some(path) => match resolve_inside(&dir, path).filter(|p| p.is_dir()) {
            Some(sub) => sub,
            None => {
                let _ = std::fs::remove_dir_all(&dir);
                bail!("Plugin source subdirectory does not exist: {path}");
            }
        },
    };
    Ok(RepoRoot { path, temp: dir })
}

/// TS `resolveRepositoryPluginSource`：Archive 失败且不属于「需要 Git 语义」时包成 archive_fetch_failed。
pub(super) async fn resolve(source: RepoSource) -> Result<RepoRoot> {
    match github_archive(&source).await {
        Ok(root) => return Ok(root),
        Err(error) if should_fallback(&error) => {}
        Err(error) => {
            return Err(SourceError {
                code: "plugin_archive_fetch_failed",
                message: format!(
                    "Failed to materialize public GitHub plugin source archive {}: {}",
                    redact_source(&source.url),
                    redact_text(&error.to_string())
                ),
            }
            .into());
        }
    }
    git_clone(&source).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    #[test]
    fn github_urls_and_redaction() {
        assert_eq!(
            public_github("https://github.com/acme/tool.git"),
            Some(("acme".into(), "tool".into()))
        );
        assert_eq!(
            public_github("https://github.com/acme/tool/tree/main"),
            None
        );
        assert_eq!(public_github("https://user:pw@github.com/acme/tool"), None);
        assert_eq!(public_github("file:///repo"), None);
        assert_eq!(
            redact_source("https://user:secret@example.com/r.git"),
            "https://example.com/r.git"
        );
        // WHATWG URL 把 `user:secret@host:repo` 解析成 scheme=user 的合法 URL（TS 同样原样返回）；
        // 只有解析失败的 `a:b@c` 形式才整体替换。
        assert_eq!(
            redact_source("user:secret@host:repo"),
            "user:secret@host:repo"
        );
        assert_eq!(
            redact_source("git@github.com:a/b.git"),
            "git@github.com:a/b.git"
        );
        assert!(retryable(&anyhow!("fatal: early EOF")));
        assert!(!retryable(&anyhow!("repository not found")));
    }
}
